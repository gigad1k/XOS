//! JSON-RPC 2.0 over a Unix socket.
//!
//! One newline-delimited JSON object per message. A completion streams as
//! `complete.delta` notifications followed by the final result, so a client can
//! render token by token without a second protocol.
//!
//! Every connection is served by its own task. That is what makes `halt`
//! dependable: a halt arriving while a completion runs is handled on a fresh
//! task, and halting takes no lock that the completion could be holding.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, error, info, warn};

use crate::providers::{CompletionRequest, ProviderRegistry};
use crate::spend::{cost_of, SpendBook};
use crate::state::Halt;
use crate::vault::Vault;

pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32000;
/// The system is halted, so nothing autonomous may run.
pub const HALTED: i32 = -32001;
/// The provider is over its daily spend cap.
pub const OVER_CAP: i32 = -32002;

#[derive(Debug, Deserialize)]
struct Request {
    #[serde(default)]
    jsonrpc: String,
    method: String,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    id: Option<Value>,
}

#[derive(Debug, Serialize)]
struct ErrorObject {
    code: i32,
    message: String,
}

/// Shared daemon state, held by every connection task.
pub struct Daemon {
    pub registry: ProviderRegistry,
    pub halt: Arc<Halt>,
    pub default_provider: String,
    pub vault: Arc<Vault>,
    pub spend: Arc<SpendBook>,
    pub version: &'static str,
}

impl Daemon {
    pub fn new(
        registry: ProviderRegistry,
        halt: Arc<Halt>,
        default_provider: String,
        vault: Arc<Vault>,
        spend: Arc<SpendBook>,
    ) -> Self {
        Self {
            registry,
            halt,
            default_provider,
            vault,
            spend,
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    /// Providers with their availability folded in, so a caller can see which
    /// are usable before committing to a request.
    fn provider_report(&self) -> Vec<Value> {
        self.registry
            .list()
            .into_iter()
            .map(|info| {
                let availability = self.spend.availability(&info.name);
                json!({
                    "name": info.name,
                    "capabilities": info.capabilities,
                    "available": availability.is_available(),
                    "unavailable_because": availability.reason(),
                    "spent_today": self.spend.spent_today(&info.name).unwrap_or(0.0),
                })
            })
            .collect()
    }
}

/// Accept connections until the process stops.
pub async fn serve(listener: UnixListener, daemon: Arc<Daemon>) {
    loop {
        match listener.accept().await {
            Ok((stream, _address)) => {
                let daemon = Arc::clone(&daemon);
                // A task per connection, so halt never queues behind a completion.
                tokio::spawn(async move {
                    if let Err(error) = handle(stream, daemon).await {
                        debug!(%error, "connection closed");
                    }
                });
            }
            Err(error) => {
                error!(%error, "cannot accept a connection");
                return;
            }
        }
    }
}

async fn handle(stream: UnixStream, daemon: Arc<Daemon>) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                let body = failure(None, PARSE_ERROR, &error.to_string());
                write_line(&mut writer, &body).await?;
                continue;
            }
        };

        if !request.jsonrpc.is_empty() && request.jsonrpc != "2.0" {
            let body = failure(
                request.id.clone(),
                INVALID_REQUEST,
                "this socket speaks JSON-RPC 2.0",
            );
            write_line(&mut writer, &body).await?;
            continue;
        }

        let id = request.id.clone();
        let response = dispatch(&request, &daemon, &mut writer).await;
        match response {
            Some(result) => {
                let body = json!({"jsonrpc": "2.0", "id": id, "result": result});
                write_line(&mut writer, &body).await?;
            }
            None => {
                // dispatch already wrote a failure, or the call was a notification.
            }
        }
    }
    Ok(())
}

/// Returns the result to send, or None when a reply was already written.
async fn dispatch(
    request: &Request,
    daemon: &Arc<Daemon>,
    writer: &mut (impl AsyncWriteExt + Unpin + Send),
) -> Option<Value> {
    match request.method.as_str() {
        "health" => Some(json!({
            "status": "ok",
            "version": daemon.version,
            "halted": daemon.halt.is_halted(),
        })),

        "status" => Some(json!({
            "version": daemon.version,
            "halted": daemon.halt.is_halted(),
            "default_provider": daemon.default_provider,
            "providers": daemon.provider_report(),
            "halt_state_path": daemon.halt.state_path().display().to_string(),
        })),

        "providers.list" => Some(json!({"providers": daemon.provider_report()})),

        // Key material goes in and never comes back out.
        "vault.set" => {
            #[derive(Deserialize)]
            struct SetParams {
                provider: String,
                key: String,
            }
            match serde_json::from_value::<SetParams>(request.params.clone()) {
                Ok(params) if params.key.trim().is_empty() => {
                    let body = failure(request.id.clone(), INVALID_PARAMS, "the key is empty");
                    let _ = write_line(writer, &body).await;
                    None
                }
                Ok(params) => match daemon
                    .vault
                    .set(&params.provider, &params.key)
                    .and_then(|()| daemon.vault.note_name(&params.provider))
                {
                    Ok(()) => {
                        // The name is safe to log. The key is not, and is not.
                        info!(provider = %params.provider, "credential stored");
                        Some(json!({"provider": params.provider, "stored": true}))
                    }
                    Err(error) => {
                        let body =
                            failure(request.id.clone(), INTERNAL_ERROR, &error.to_string());
                        let _ = write_line(writer, &body).await;
                        None
                    }
                },
                Err(error) => {
                    let body = failure(request.id.clone(), INVALID_PARAMS, &error.to_string());
                    let _ = write_line(writer, &body).await;
                    None
                }
            }
        }

        "vault.list" => match daemon.vault.list() {
            Ok(providers) => Some(json!({
                "providers": providers,
                "store": daemon.vault.backend().label(),
            })),
            Err(error) => {
                let body = failure(request.id.clone(), INTERNAL_ERROR, &error.to_string());
                let _ = write_line(writer, &body).await;
                None
            }
        },

        "vault.remove" => {
            let provider = request
                .params
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            match daemon
                .vault
                .remove(&provider)
                .and_then(|()| daemon.vault.forget_name(&provider))
            {
                Ok(()) => Some(json!({"provider": provider, "removed": true})),
                Err(error) => {
                    let body = failure(request.id.clone(), INVALID_PARAMS, &error.to_string());
                    let _ = write_line(writer, &body).await;
                    None
                }
            }
        }

        "spend.report" => {
            let days = request
                .params
                .get("days")
                .and_then(Value::as_u64)
                .unwrap_or(7) as u32;
            match daemon.spend.by_day(days) {
                Ok(rows) => Some(json!({
                    "days": days,
                    "rows": rows,
                    "today_total": daemon.spend.spent_today_total().unwrap_or(0.0),
                    "caps": daemon.spend.caps(),
                })),
                Err(error) => {
                    let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
                    let _ = write_line(writer, &body).await;
                    None
                }
            }
        }

        "halt" => {
            // Deliberately the shortest path in the file.
            match daemon.halt.halt() {
                Ok(()) => {
                    warn!("halted: every autonomous action is stopped");
                    Some(json!({"halted": true}))
                }
                Err(error) => {
                    // The flag is already set in memory; only persistence failed.
                    let body = failure(
                        request.id.clone(),
                        INTERNAL_ERROR,
                        &format!("halted, but the flag was not persisted: {}", error),
                    );
                    let _ = write_line(writer, &body).await;
                    None
                }
            }
        }

        "resume" => match daemon.halt.resume() {
            Ok(()) => {
                info!("resumed");
                Some(json!({"halted": false}))
            }
            Err(error) => {
                let body = failure(request.id.clone(), INTERNAL_ERROR, &error.to_string());
                let _ = write_line(writer, &body).await;
                None
            }
        },

        "complete" => complete(request, daemon, writer).await,

        other => {
            let body = failure(
                request.id.clone(),
                METHOD_NOT_FOUND,
                &format!("no method `{}`", other),
            );
            let _ = write_line(writer, &body).await;
            None
        }
    }
}

#[derive(Debug, Deserialize)]
struct CompleteParams {
    #[serde(default)]
    provider: Option<String>,
    #[serde(flatten)]
    request: CompletionRequest,
}

async fn complete(
    request: &Request,
    daemon: &Arc<Daemon>,
    writer: &mut (impl AsyncWriteExt + Unpin + Send),
) -> Option<Value> {
    // The same guard every future subsystem calls before acting.
    if daemon.halt.guard().is_err() {
        let body = failure(
            request.id.clone(),
            HALTED,
            "the system is halted. Run `xos resume` to start it again.",
        );
        let _ = write_line(writer, &body).await;
        return None;
    }

    let params: CompleteParams = match serde_json::from_value(request.params.clone()) {
        Ok(params) => params,
        Err(error) => {
            let body = failure(request.id.clone(), INVALID_PARAMS, &error.to_string());
            let _ = write_line(writer, &body).await;
            return None;
        }
    };

    let name = params
        .provider
        .clone()
        .unwrap_or_else(|| daemon.default_provider.clone());
    let provider = match daemon.registry.get(&name) {
        Some(provider) => provider,
        None => {
            let body = failure(
                request.id.clone(),
                INVALID_PARAMS,
                &format!("no provider named `{}`", name),
            );
            let _ = write_line(writer, &body).await;
            return None;
        }
    };

    // Checked before the request rather than during it, so a cap never
    // interrupts a reply that is already streaming.
    if let Some(reason) = daemon.spend.availability(&name).reason() {
        let body = failure(request.id.clone(), OVER_CAP, &reason);
        let _ = write_line(writer, &body).await;
        return None;
    }

    let mut stream = match provider.complete(params.request).await {
        Ok(stream) => stream,
        Err(error) => {
            let body = failure(request.id.clone(), INTERNAL_ERROR, &error.to_string());
            let _ = write_line(writer, &body).await;
            return None;
        }
    };

    let mut halted = daemon.halt.subscribe();
    let mut text = String::new();
    let mut finish_reason = None;
    let mut usage = None;

    loop {
        tokio::select! {
            // Halt wins the race by construction: it is checked on every
            // iteration and needs no lock the stream could be holding.
            _ = halted.recv() => {
                let body = failure(
                    request.id.clone(),
                    HALTED,
                    "halted mid-completion",
                );
                let _ = write_line(writer, &body).await;
                return None;
            }
            next = futures_util::StreamExt::next(&mut stream) => {
                match next {
                    Some(Ok(token)) => {
                        if !token.text.is_empty() {
                            text.push_str(&token.text);
                            let note = json!({
                                "jsonrpc": "2.0",
                                "method": "complete.delta",
                                "params": {"id": request.id, "text": token.text}
                            });
                            if write_line(writer, &note).await.is_err() {
                                return None;
                            }
                        }
                        if token.finish_reason.is_some() {
                            finish_reason = token.finish_reason;
                        }
                        if token.usage.is_some() {
                            usage = token.usage;
                        }
                    }
                    Some(Err(error)) => {
                        let body = failure(request.id.clone(), INTERNAL_ERROR, &error.to_string());
                        let _ = write_line(writer, &body).await;
                        return None;
                    }
                    None => break,
                }
            }
        }
    }

    // Record what it cost before replying, so a cap reflects this call too.
    let capabilities = provider.capabilities();
    let charged = usage.map(|usage| {
        let cost = cost_of(
            usage.input_tokens,
            usage.output_tokens,
            capabilities.cost_per_1k_input,
            capabilities.cost_per_1k_output,
        );
        if let Err(error) =
            daemon
                .spend
                .record(&name, usage.input_tokens, usage.output_tokens, cost)
        {
            warn!(%error, "the completion finished but its spend was not recorded");
        }
        cost
    });

    Some(json!({
        "provider": name,
        "text": text,
        "finish_reason": finish_reason,
        "usage": usage,
        "cost": charged,
    }))
}

fn failure(id: Option<Value>, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": ErrorObject { code, message: message.to_string() }
    })
}

async fn write_line(
    writer: &mut (impl AsyncWriteExt + Unpin + Send),
    value: &Value,
) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await
}
