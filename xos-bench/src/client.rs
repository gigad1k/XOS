//! OpenAI-compatible chat client.
//!
//! Talks to llama.cpp server, Ollama or any endpoint exposing
//! `/chat/completions`. Read-only: it sends requests and reads replies, and
//! never writes to the endpoint's host.

use std::io::{BufRead, BufReader};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

/// One tool call emitted by the model.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw argument text. Left unparsed so a malformed payload stays visible.
    pub arguments: String,
}

/// What one request produced, including the timings the report needs.
#[derive(Debug, Clone)]
pub struct Completion {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub ttft_ms: Option<u128>,
    pub latency_ms: u128,
    pub tokens_in: Option<u32>,
    pub tokens_out: Option<u32>,
    /// True when token counts were estimated because the endpoint omitted usage.
    pub tokens_estimated: bool,
}

/// A message in the conversation sent to the endpoint.
#[derive(Debug, Clone)]
pub enum Turn {
    User(String),
    /// The assistant's tool call, replayed so the model sees its own move.
    AssistantCall(ToolCall),
    /// The result handed back for a tool call.
    ToolResult { call_id: String, content: String },
}

pub struct Client {
    base_url: String,
    model: String,
    api_key: Option<String>,
    timeout: Duration,
    stream: bool,
}

impl Client {
    pub fn new(
        base_url: &str,
        model: &str,
        api_key: Option<String>,
        timeout: Duration,
        stream: bool,
    ) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            api_key,
            timeout,
            stream,
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    /// Send one turn set and read the reply.
    pub fn complete(&self, turns: &[Turn], tools: &[Value]) -> Result<Completion, String> {
        let mut messages: Vec<Value> = Vec::new();
        for turn in turns {
            match turn {
                Turn::User(text) => messages.push(json!({"role": "user", "content": text})),
                Turn::AssistantCall(call) => messages.push(json!({
                    "role": "assistant",
                    "content": Value::Null,
                    "tool_calls": [{
                        "id": call.id,
                        "type": "function",
                        "function": {"name": call.name, "arguments": call.arguments}
                    }]
                })),
                Turn::ToolResult { call_id, content } => messages.push(json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": content
                })),
            }
        }

        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "temperature": 0.0,
            "stream": self.stream,
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
            body["tool_choice"] = Value::String("auto".to_string());
        }
        if self.stream {
            body["stream_options"] = json!({"include_usage": true});
        }

        let started = Instant::now();
        let mut request = ureq::post(&self.endpoint())
            .set("Content-Type", "application/json")
            .timeout(self.timeout);
        if let Some(key) = &self.api_key {
            request = request.set("Authorization", &format!("Bearer {}", key));
        }

        let response = request.send_string(&body.to_string()).map_err(|e| match e {
            ureq::Error::Status(code, resp) => {
                let detail = resp.into_string().unwrap_or_default();
                let detail: String = detail.chars().take(200).collect();
                format!("http {}: {}", code, detail.trim())
            }
            other => format!("transport: {}", other),
        })?;

        if self.stream {
            self.read_stream(response, started)
        } else {
            self.read_whole(response, started)
        }
    }

    fn read_whole(&self, response: ureq::Response, started: Instant) -> Result<Completion, String> {
        let text = response
            .into_string()
            .map_err(|e| format!("read body: {}", e))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|e| format!("decode body: {} (body started {})", e, snippet(&text)))?;
        let latency_ms = started.elapsed().as_millis();

        let message = value
            .pointer("/choices/0/message")
            .ok_or_else(|| format!("no choices in reply: {}", snippet(&text)))?;
        let content = message
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let mut calls = Vec::new();
        if let Some(array) = message.get("tool_calls").and_then(Value::as_array) {
            for (index, call) in array.iter().enumerate() {
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("call_{}", index));
                calls.push(ToolCall {
                    id,
                    name: call
                        .pointer("/function/name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    arguments: argument_text(call.pointer("/function/arguments")),
                });
            }
        }

        let (tokens_in, tokens_out) = usage_of(&value);
        let estimated = tokens_out.is_none();
        let fallback = estimate_tokens(&content, &calls);
        Ok(Completion {
            tokens_out: tokens_out.or(Some(fallback)),
            tokens_in,
            tokens_estimated: estimated,
            content,
            tool_calls: calls,
            ttft_ms: None,
            latency_ms,
        })
    }

    fn read_stream(&self, response: ureq::Response, started: Instant) -> Result<Completion, String> {
        let reader = BufReader::new(response.into_reader());
        let mut content = String::new();
        // Tool call fragments arrive spread across chunks, keyed by index.
        let mut partials: Vec<(String, String, String)> = Vec::new();
        let mut ttft_ms: Option<u128> = None;
        let mut tokens_in = None;
        let mut tokens_out = None;

        for line in reader.lines() {
            let line = line.map_err(|e| format!("read stream: {}", e))?;
            let payload = match line.strip_prefix("data:") {
                Some(rest) => rest.trim(),
                None => continue,
            };
            if payload == "[DONE]" {
                break;
            }
            let chunk: Value = match serde_json::from_str(payload) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let (prompt_tokens, completion_tokens) = usage_of(&chunk);
            if prompt_tokens.is_some() {
                tokens_in = prompt_tokens;
            }
            if completion_tokens.is_some() {
                tokens_out = completion_tokens;
            }
            let delta = match chunk.pointer("/choices/0/delta") {
                Some(delta) => delta,
                None => continue,
            };
            if let Some(text) = delta.get("content").and_then(Value::as_str) {
                if !text.is_empty() {
                    if ttft_ms.is_none() {
                        ttft_ms = Some(started.elapsed().as_millis());
                    }
                    content.push_str(text);
                }
            }
            if let Some(array) = delta.get("tool_calls").and_then(Value::as_array) {
                if ttft_ms.is_none() {
                    ttft_ms = Some(started.elapsed().as_millis());
                }
                for call in array {
                    let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    while partials.len() <= index {
                        partials.push((String::new(), String::new(), String::new()));
                    }
                    let slot = &mut partials[index];
                    if let Some(id) = call.get("id").and_then(Value::as_str) {
                        if !id.is_empty() {
                            slot.0 = id.to_string();
                        }
                    }
                    if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                        if !name.is_empty() {
                            slot.1 = name.to_string();
                        }
                    }
                    if let Some(args) = call.pointer("/function/arguments") {
                        slot.2.push_str(&argument_text(Some(args)));
                    }
                }
            }
        }

        let latency_ms = started.elapsed().as_millis();
        let calls: Vec<ToolCall> = partials
            .into_iter()
            .enumerate()
            .filter(|(_, (_, name, args))| !name.is_empty() || !args.is_empty())
            .map(|(index, (id, name, arguments))| ToolCall {
                id: if id.is_empty() {
                    format!("call_{}", index)
                } else {
                    id
                },
                name,
                arguments,
            })
            .collect();

        let estimated = tokens_out.is_none();
        let fallback = estimate_tokens(&content, &calls);
        Ok(Completion {
            tokens_out: tokens_out.or(Some(fallback)),
            tokens_in,
            tokens_estimated: estimated,
            content,
            tool_calls: calls,
            ttft_ms,
            latency_ms,
        })
    }
}

/// Arguments may arrive as a JSON string or as an object. Normalise to text.
fn argument_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Object(map)) => Value::Object(Map::clone(map)).to_string(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn usage_of(value: &Value) -> (Option<u32>, Option<u32>) {
    let usage = match value.get("usage") {
        Some(Value::Object(map)) => map,
        _ => return (None, None),
    };
    let read = |key: &str| usage.get(key).and_then(Value::as_u64).map(|n| n as u32);
    (read("prompt_tokens"), read("completion_tokens"))
}

/// Rough fallback when the endpoint reports no usage. Four characters per token
/// is the usual approximation; results carry a flag saying the figure is one.
fn estimate_tokens(content: &str, calls: &[ToolCall]) -> u32 {
    let mut characters = content.chars().count();
    for call in calls {
        characters += call.name.chars().count() + call.arguments.chars().count();
    }
    ((characters as f64) / 4.0).ceil() as u32
}

fn snippet(text: &str) -> String {
    text.chars().take(120).collect()
}
