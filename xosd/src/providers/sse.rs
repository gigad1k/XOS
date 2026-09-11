//! Reading server-sent events from a streaming HTTP reply.
//!
//! Every provider XOS talks to streams over SSE, and they differ only in the
//! shape of the JSON inside each event. This handles the part that is the same:
//! chunk boundaries can split a line, so only whole lines are parsed, and
//! `[DONE]` ends the stream.

use futures_util::StreamExt;
use serde_json::Value;

use super::{ProviderError, Token, TokenStream};

/// Turn a streaming response into tokens, using `parse` for the provider's own
/// event shape. Returning `None` from `parse` skips an event.
pub fn read<F>(response: reqwest::Response, mut parse: F) -> TokenStream
where
    F: FnMut(&Value) -> Option<Token> + Send + 'static,
{
    let mut bytes = response.bytes_stream();
    Box::pin(async_stream::stream! {
        let mut buffer = String::new();
        while let Some(chunk) = bytes.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    yield Err(ProviderError::Transport(error.to_string()));
                    return;
                }
            };
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(position) = buffer.find('\n') {
                let line = buffer[..position].trim().to_string();
                buffer.drain(..=position);

                let payload = match line.strip_prefix("data:") {
                    Some(rest) => rest.trim(),
                    None => continue,
                };
                if payload == "[DONE]" {
                    return;
                }
                let event: Value = match serde_json::from_str(payload) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                if let Some(token) = parse(&event) {
                    yield Ok(token);
                }
            }
        }
    })
}

/// Read usage from an OpenAI-shaped event.
pub fn openai_usage(event: &Value) -> Option<super::Usage> {
    let usage = event.get("usage")?.as_object()?;
    let read = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0) as u32;
    Some(super::Usage {
        input_tokens: read("prompt_tokens"),
        output_tokens: read("completion_tokens"),
        cached_tokens: usage
            .get("prompt_tokens_details")
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
    })
}

/// Turn one OpenAI-shaped chunk into a token, or nothing when it carries none.
pub fn openai_token(event: &Value) -> Option<Token> {
    let usage = openai_usage(event);
    let finish_reason = event
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
        .map(str::to_string);
    let text = event
        .pointer("/choices/0/delta/content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let logprob = event
        .pointer("/choices/0/logprobs/content/0/logprob")
        .and_then(Value::as_f64)
        .map(|value| value as f32);

    if text.is_empty() && finish_reason.is_none() && usage.is_none() {
        return None;
    }
    Some(Token {
        text,
        logprob,
        finish_reason,
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_content_delta_becomes_a_token() {
        let event = json!({"choices":[{"delta":{"content":"hel"}}]});
        let token = openai_token(&event).expect("a token");
        assert_eq!(token.text, "hel");
        assert!(token.usage.is_none());
    }

    #[test]
    fn an_empty_event_is_skipped() {
        assert!(openai_token(&json!({"choices":[{"delta":{}}]})).is_none());
    }

    #[test]
    fn usage_arrives_as_a_final_token() {
        let event = json!({"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":7}});
        let token = openai_token(&event).expect("a token");
        let usage = token.usage.expect("usage");
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.output_tokens, 7);
    }

    #[test]
    fn a_logprob_is_read_when_the_endpoint_reports_one() {
        let event = json!({"choices":[{"delta":{"content":"x"},
                           "logprobs":{"content":[{"logprob":-0.35}]}}]});
        let token = openai_token(&event).expect("a token");
        assert!((token.logprob.expect("logprob") + 0.35).abs() < 1e-6);
    }

    #[test]
    fn a_missing_logprob_is_simply_absent() {
        let event = json!({"choices":[{"delta":{"content":"x"}}]});
        assert!(openai_token(&event).expect("a token").logprob.is_none());
    }

    #[test]
    fn cached_tokens_are_read_when_reported() {
        let event = json!({"usage":{"prompt_tokens":100,"completion_tokens":5,
                                    "prompt_tokens_details":{"cached_tokens":80}}});
        let usage = openai_usage(&event).expect("usage");
        assert_eq!(usage.cached_tokens, 80);
    }
}
