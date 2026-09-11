//! JSON-RPC 2.0 over a Unix socket.
//!
//! One newline-delimited JSON object per message. A completion streams as
//! `complete.delta` notifications followed by the final result, so a client can
//! render token by token without a second protocol.
//!
//! Every connection is served by its own task. That is what makes `halt`
//! dependable: a halt arriving while a completion runs is handled on a fresh
//! task, and halting takes no lock that the completion could be holding.

use std::sync::{Arc, RwLock};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, error, info, warn};

use crate::journal::{Filter as JournalFilter, Journal};
use crate::memory::{bundle, Memory, Tier};
use crate::policy::log::PolicyLog;
use crate::policy::{
    digest, redact, Context as PolicyContext, Permit, Policy, PolicyDecision,
};
use crate::providers::{CompletionRequest, Message, ProviderRegistry};
use crate::router::log::{EscalationLog, Record};
use crate::router::{self, CostMode, Observation, Router, Tier as RouteTier};
use crate::spend::{cost_of, SpendBook};
use crate::state::Halt;
use crate::tools;
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
    /// Behind a lock only because `xos mode` can change it at runtime. Routing
    /// itself takes a read, which never blocks another completion.
    pub router: RwLock<Router>,
    pub escalations: Arc<EscalationLog>,
    pub memory: Arc<Memory>,
    pub policy: Arc<Policy>,
    pub policy_log: Arc<PolicyLog>,
    pub journal: Arc<Journal>,
    pub version: &'static str,
}

impl Daemon {
    pub fn new(
        registry: ProviderRegistry,
        halt: Arc<Halt>,
        default_provider: String,
        vault: Arc<Vault>,
        spend: Arc<SpendBook>,
        router: Router,
        escalations: Arc<EscalationLog>,
        memory: Arc<Memory>,
        policy: Arc<Policy>,
        policy_log: Arc<PolicyLog>,
        journal: Arc<Journal>,
    ) -> Self {
        Self {
            registry,
            halt,
            default_provider,
            vault,
            spend,
            router: RwLock::new(router),
            escalations,
            memory,
            policy,
            policy_log,
            journal,
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    fn cost_mode(&self) -> CostMode {
        self.router
            .read()
            .map(|router| router.config().cost_mode)
            .unwrap_or_default()
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
            "cost_mode": daemon.cost_mode().label(),
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

        // Judge a call without running it. This is `xos policy test`.
        // The only path by which XOS touches a disk.
        "tool.call" => {
            match serde_json::from_value::<tools::ToolCall>(request.params.clone()) {
                Ok(call) => {
                    let context = PolicyContext {
                        untrusted_source: request
                            .params
                            .get("untrusted")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        local_only: daemon.cost_mode() == CostMode::AggressiveLocal,
                        ..Default::default()
                    };
                    let outcome =
                        tools::run(&call, &daemon.policy, &daemon.journal, &context);
                    let _ = daemon.policy_log.record(
                        &call.tool,
                        &call.arguments.to_string(),
                        &outcome.decision,
                        "tool.call",
                    );
                    Some(tools::outcome_json(&outcome))
                }
                Err(error) => {
                    let body = failure(request.id.clone(), INVALID_PARAMS, &error.to_string());
                    let _ = write_line(writer, &body).await;
                    None
                }
            }
        }

        "journal.list" => {
            let filter = JournalFilter {
                goal: request
                    .params
                    .get("goal")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                tool: request
                    .params
                    .get("tool")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                since: request.params.get("since").and_then(Value::as_i64),
                until: request.params.get("until").and_then(Value::as_i64),
                limit: request
                    .params
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(20) as u32,
            };
            match daemon.journal.list(&filter) {
                Ok(entries) => Some(json!({"entries": entries})),
                Err(error) => {
                    let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
                    let _ = write_line(writer, &body).await;
                    None
                }
            }
        }

        "journal.undo" => {
            let count = request
                .params
                .get("last")
                .and_then(Value::as_u64)
                .unwrap_or(1) as usize;
            match daemon.journal.undo(count) {
                Ok(report) => {
                    if let Some(reason) = &report.refused {
                        warn!(%reason, "an undo was refused");
                    } else {
                        info!(undone = report.undone.len(), "undone");
                    }
                    Some(serde_json::to_value(report).unwrap_or(Value::Null))
                }
                Err(error) => {
                    let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
                    let _ = write_line(writer, &body).await;
                    None
                }
            }
        }

        "policy.test" => {
            let tool = request
                .params
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let arguments = request
                .params
                .get("arguments")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            let context = PolicyContext {
                api_bound: request
                    .params
                    .get("api_bound")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                untrusted_source: request
                    .params
                    .get("untrusted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                local_only: daemon.cost_mode() == CostMode::AggressiveLocal,
            };

            let decision = daemon.policy.evaluate(tool, &arguments, &context);
            let _ = daemon.policy_log.record(
                tool,
                &arguments.to_string(),
                &decision,
                "policy.test",
            );
            // A permit is the only thing that lets a tool run, so reporting
            // whether one would be issued answers the question people actually
            // have: would this go ahead?
            let permit = Permit::issue(tool, decision.clone());
            Some(json!({
                "tool": tool,
                "decision": decision.label(),
                "reason": decision.reason(),
                "runs": decision.runs(),
                "permit_issued": permit.as_ref().map(|permit| permit.tool()),
            }))
        }

        "policy.log" => {
            let limit = request
                .params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20) as u32;
            match daemon.policy_log.recent(limit) {
                Ok(entries) => Some(json!({
                    "entries": entries,
                    "strictness": daemon.policy.config().strictness.label(),
                })),
                Err(error) => {
                    let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
                    let _ = write_line(writer, &body).await;
                    None
                }
            }
        }

        // Build the bounded, redacted packet the supervisor tier receives.
        "policy.digest" => {
            let pieces: Vec<digest::Piece> = request
                .params
                .get("pieces")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|item| {
                            digest::Piece::new(
                                item.get("source").and_then(Value::as_str).unwrap_or("unknown"),
                                item.get("content").and_then(Value::as_str).unwrap_or(""),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let cap = request
                .params
                .get("token_cap")
                .and_then(Value::as_u64)
                .unwrap_or(digest::TOKEN_CAP as u64) as usize;
            Some(serde_json::to_value(digest::build(&pieces, cap)).unwrap_or(Value::Null))
        }

        "memory.write" => {
            let tier = request
                .params
                .get("tier")
                .and_then(Value::as_str)
                .and_then(Tier::parse)
                .unwrap_or(Tier::Working);
            let content = request
                .params
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let tags = request
                .params
                .get("tags")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let source = request
                .params
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("rpc");
            match daemon.memory.write(tier, content, tags, source) {
                Ok(id) => Some(json!({"id": id, "tier": tier.label()})),
                Err(error) => {
                    let body = failure(request.id.clone(), INVALID_PARAMS, &error);
                    let _ = write_line(writer, &body).await;
                    None
                }
            }
        }

        "memory.recall" => {
            let query = request
                .params
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let tier = request
                .params
                .get("tier")
                .and_then(Value::as_str)
                .and_then(Tier::parse);
            let limit = request
                .params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(10) as usize;
            match daemon.memory.recall(query, tier, limit) {
                Ok(hits) => Some(json!({"hits": hits})),
                Err(error) => {
                    let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
                    let _ = write_line(writer, &body).await;
                    None
                }
            }
        }

        "memory.stats" => match daemon.memory.stats() {
            Ok(stats) => Some(json!({
                "tiers": stats,
                "embedder": daemon.memory.embedder().label(),
            })),
            Err(error) => {
                let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
                let _ = write_line(writer, &body).await;
                None
            }
        },

        "memory.promote" => promote(request, daemon, writer).await,

        "memory.export" => {
            let path = request
                .params
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let passphrase = request
                .params
                .get("passphrase")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let entries = daemon.memory.all().unwrap_or_default();
            let count = entries.len();
            // Names only. The vault is not read here, and could not be put in
            // the bundle if it were.
            let providers = daemon.vault.list().unwrap_or_default();
            let bundle = bundle::Bundle::new(entries, Value::Null, providers);
            match bundle::export(std::path::Path::new(path), &bundle, passphrase) {
                Ok(()) => {
                    info!(path = %path, entries = count, "exported");
                    Some(json!({"path": path, "entries": count}))
                }
                Err(error) => {
                    let body = failure(request.id.clone(), INVALID_PARAMS, &error);
                    let _ = write_line(writer, &body).await;
                    None
                }
            }
        }

        "memory.import" => {
            let path = request
                .params
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let passphrase = request
                .params
                .get("passphrase")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match bundle::import(std::path::Path::new(path), passphrase) {
                Ok(bundle) => {
                    let offered = bundle.memory.len();
                    match daemon.memory.merge(&bundle.memory) {
                        Ok(added) => {
                            info!(path = %path, added, "imported");
                            Some(json!({
                                "offered": offered,
                                "added": added,
                                "kept": offered - added,
                                "providers_to_reconnect": bundle.vault_providers,
                            }))
                        }
                        Err(error) => {
                            let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
                            let _ = write_line(writer, &body).await;
                            None
                        }
                    }
                }
                Err(error) => {
                    let body = failure(request.id.clone(), INVALID_PARAMS, &error);
                    let _ = write_line(writer, &body).await;
                    None
                }
            }
        }

        "escalations.list" => {
            let limit = request
                .params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20) as u32;
            match daemon.escalations.recent(limit) {
                Ok(entries) => Some(json!({"entries": entries})),
                Err(error) => {
                    let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
                    let _ = write_line(writer, &body).await;
                    None
                }
            }
        }

        "mode.get" => Some(json!({"mode": daemon.cost_mode().label()})),

        "mode.set" => {
            let wanted = request
                .params
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match CostMode::parse(wanted) {
                Some(mode) => {
                    {
                        // Scoped tightly: the guard must not outlive this block.
                        if let Ok(mut router) = daemon.router.write() {
                            router.set_cost_mode(mode);
                        }
                    }
                    match crate::config::write_mode(mode) {
                        Ok(()) => {
                            info!(mode = mode.label(), "cost mode set");
                            Some(json!({"mode": mode.label()}))
                        }
                        Err(error) => {
                            let body = failure(
                                request.id.clone(),
                                INTERNAL_ERROR,
                                &format!("the mode is set, but was not remembered: {}", error),
                            );
                            let _ = write_line(writer, &body).await;
                            None
                        }
                    }
                }
                None => {
                    let body = failure(
                        request.id.clone(),
                        INVALID_PARAMS,
                        "modes are aggressive-local, balanced and best-quality",
                    );
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
    /// Naming a provider bypasses routing entirely.
    #[serde(default)]
    provider: Option<String>,
    /// Ask for the API tier outright. Never throttled.
    #[serde(default)]
    escalate: bool,
    /// What the caller already knows about the work, when it knows.
    #[serde(default)]
    task_class: Option<router::TaskClass>,
    /// Local attempts that already failed schema validation.
    #[serde(default)]
    schema_failures: u32,
    #[serde(flatten)]
    request: CompletionRequest,
}

/// What one streaming attempt produced.
struct Attempt {
    text: String,
    finish_reason: Option<String>,
    usage: Option<crate::providers::Usage>,
    /// Set when the router asked to move tiers partway through.
    escalation: Option<crate::router::Decision>,
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

    // Routing happens here and only here. Providers know nothing about tiers.
    // One snapshot, taken without holding the lock across any await.
    let snapshot = daemon.router.read().ok().map(|router| router.clone());
    let routing = {
        let router = match snapshot.as_ref() {
            Some(router) => router,
            None => {
                let body = failure(request.id.clone(), INTERNAL_ERROR, "the router is unavailable");
                let _ = write_line(writer, &body).await;
                return None;
            }
        };
        let prompt = params
            .request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .map(|message| message.content.clone())
            .unwrap_or_default();
        let whole: String = params
            .request
            .messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        let decision = router.route(&router::Request {
            prompt,
            task_class: params.task_class,
            tools_offered: params.request.tools.len(),
            estimated_tokens: router::estimate_tokens(&whole),
            schema_failures: params.schema_failures,
            manual: params.escalate,
        });
        let local = router.config().local.clone();
        let api = router.config().api.clone();
        (decision, local, api)
    };
    let (decision, local_name, api_name) = routing;

    // An explicitly named provider wins: the caller has already decided.
    let mut name = match (&params.provider, decision.tier) {
        (Some(explicit), _) => explicit.clone(),
        (None, RouteTier::Api) => match &api_name {
            Some(api) => api.clone(),
            None => local_name.clone(),
        },
        (None, RouteTier::Local) => local_name.clone(),
    };
    if params.provider.is_none() && decision.escalated() {
        info!(
            trigger = decision.trigger.map(|t| t.label()).unwrap_or("none"),
            reason = %decision.reason,
            target = %name,
            "escalating to the API tier"
        );
    }

    let mut escalated_by = if params.provider.is_none() && decision.escalated() {
        decision.trigger
    } else {
        None
    };
    let mut escalation_reason = decision.reason.clone();

    let mut attempt = match run_attempt(request, daemon, &name, &params, &decision, writer).await {
        Ok(attempt) => attempt,
        Err(()) => return None,
    };

    // A mid-stream escalation: stop the local reply and run the API tier.
    if let Some(mid) = attempt.escalation.clone() {
        if let Some(api) = &api_name {
            let note = json!({
                "jsonrpc": "2.0",
                "method": "complete.escalated",
                "params": {
                    "id": request.id,
                    "trigger": mid.trigger.map(|t| t.label()),
                    "reason": mid.reason,
                }
            });
            if write_line(writer, &note).await.is_err() {
                return None;
            }
            escalated_by = mid.trigger;
            escalation_reason = mid.reason.clone();
            name = api.clone();
            attempt = match run_attempt(request, daemon, &name, &params, &decision, writer).await {
                Ok(attempt) => attempt,
                Err(()) => return None,
            };
        }
    }

    let provider = match daemon.registry.get(&name) {
        Some(provider) => provider,
        None => return None,
    };
    let capabilities = provider.capabilities();

    // Record what it cost before replying, so a cap reflects this call too.
    let charged = attempt.usage.map(|usage| {
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

    // Every escalation is auditable, with the trigger that caused it.
    if let Some(trigger) = escalated_by {
        let usage = attempt.usage.unwrap_or_default();
        let entry = Record {
            trigger,
            task_class: decision.task_class,
            local_model: local_name.clone(),
            target_model: name.clone(),
            tokens_in: usage.input_tokens,
            tokens_out: usage.output_tokens,
            cost: charged.unwrap_or(0.0),
            outcome: if attempt.text.is_empty() {
                "empty".to_string()
            } else {
                "ok".to_string()
            },
            reason: escalation_reason.clone(),
        };
        if let Err(error) = daemon.escalations.record(&entry) {
            warn!(%error, "the escalation happened but was not logged");
        }
    }

    Some(json!({
        "provider": name,
        "text": attempt.text,
        "finish_reason": attempt.finish_reason,
        "usage": attempt.usage,
        "cost": charged,
        "escalated": escalated_by.map(|t| t.label()),
        "task_class": decision.task_class.label(),
    }))
}

/// Stream one provider's reply, watching whether the router wants to move.
///
/// `Err(())` means a reply was already written and the caller should stop.
async fn run_attempt(
    request: &Request,
    daemon: &Arc<Daemon>,
    name: &str,
    params: &CompleteParams,
    decision: &crate::router::Decision,
    writer: &mut (impl AsyncWriteExt + Unpin + Send),
) -> Result<Attempt, ()> {
    let provider = match daemon.registry.get(name) {
        Some(provider) => provider,
        None => {
            let body = failure(
                request.id.clone(),
                INVALID_PARAMS,
                &format!("no provider named `{}`", name),
            );
            let _ = write_line(writer, &body).await;
            return Err(());
        }
    };

    // Checked before the request rather than during it, so a cap never
    // interrupts a reply that is already streaming.
    if let Some(reason) = daemon.spend.availability(name).reason() {
        let body = failure(request.id.clone(), OVER_CAP, &reason);
        let _ = write_line(writer, &body).await;
        return Err(());
    }

    // Egress protection. A local provider keeps everything on the machine, so
    // nothing needs removing; a cloud provider does not, so every message is
    // scanned first. This is the axis reversibility misses: reading a file is
    // reversible, and the secret inside it leaving is not.
    let mut outgoing = params.request.clone();
    if !provider.capabilities().local {
        let egress = PolicyContext {
            api_bound: true,
            ..Default::default()
        };
        // Keep the spans, not just a count: the log has to be able to say what
        // was removed, and a security record that undercounts is worse than none.
        let mut removed_spans = Vec::new();
        for message in outgoing.messages.iter_mut() {
            if let PolicyDecision::Redact { spans } =
                daemon.policy.evaluate_result(&message.content, &egress)
            {
                message.content = redact::apply(&message.content, &spans);
                removed_spans.extend(spans);
            }
        }
        let removed = removed_spans.len();
        if removed > 0 {
            let decision = PolicyDecision::Redact {
                spans: removed_spans,
            };
            warn!(
                provider = %name,
                removed,
                "credential-shaped strings removed before leaving the machine"
            );
            let _ = daemon.policy_log.record(
                "complete",
                &format!("{} messages bound for {}", outgoing.messages.len(), name),
                &decision,
                "egress",
            );
        }
    }

    let mut stream = match provider.complete(outgoing).await {
        Ok(stream) => stream,
        Err(error) => {
            let body = failure(request.id.clone(), INTERNAL_ERROR, &error.to_string());
            let _ = write_line(writer, &body).await;
            return Err(());
        }
    };

    let local_tier = provider.capabilities().local;
    // Cloned up front: holding the lock across an await would make this future
    // non-Send, and a completion must never pin the router anyway.
    let watcher = if local_tier {
        daemon.router.read().ok().map(|router| router.clone())
    } else {
        None
    };
    let mut halted = daemon.halt.subscribe();
    let mut attempt = Attempt {
        text: String::new(),
        finish_reason: None,
        usage: None,
        escalation: None,
    };
    let started = Instant::now();
    let mut tokens = 0u32;
    let mut logprob_total = 0f32;
    let mut logprob_count = 0u32;
    let mut first_token_seen = false;

    loop {
        tokio::select! {
            // Halt wins the race by construction: it is checked on every
            // iteration and needs no lock the stream could be holding.
            _ = halted.recv() => {
                let body = failure(request.id.clone(), HALTED, "halted mid-completion");
                let _ = write_line(writer, &body).await;
                return Err(());
            }
            next = futures_util::StreamExt::next(&mut stream) => {
                match next {
                    Some(Ok(token)) => {
                        if !token.text.is_empty() {
                            first_token_seen = true;
                            tokens += 1;
                            attempt.text.push_str(&token.text);
                            let note = json!({
                                "jsonrpc": "2.0",
                                "method": "complete.delta",
                                "params": {"id": request.id, "text": token.text}
                            });
                            if write_line(writer, &note).await.is_err() {
                                return Err(());
                            }
                        }
                        if let Some(logprob) = token.logprob {
                            logprob_total += logprob;
                            logprob_count += 1;
                        }
                        if token.finish_reason.is_some() {
                            attempt.finish_reason = token.finish_reason;
                        }
                        if token.usage.is_some() {
                            attempt.usage = token.usage;
                        }

                        // Only a local reply is worth moving; the API tier is
                        // already the destination.
                        if let (Some(watcher), None) = (&watcher, &attempt.escalation) {
                            let observation = Observation {
                                elapsed: started.elapsed(),
                                tokens,
                                first_token_seen,
                                mean_logprob: if logprob_count > 0 {
                                    Some(logprob_total / logprob_count as f32)
                                } else {
                                    None
                                },
                                logprob_samples: logprob_count,
                                schema_failed: false,
                            };
                            attempt.escalation =
                                watcher.reconsider(decision.task_class, &observation);
                            if attempt.escalation.is_some() {
                                break;
                            }
                        }
                    }
                    Some(Err(error)) => {
                        let body = failure(request.id.clone(), INTERNAL_ERROR, &error.to_string());
                        let _ = write_line(writer, &body).await;
                        return Err(());
                    }
                    None => break,
                }
            }
        }
    }

    Ok(attempt)
}

/// Promote one tier into the next, summarising with the local model.
///
/// Closing a task summarises working into session; closing a day distils
/// session into long-term. The summary is made on this machine, so promotion
/// costs nothing and sends nothing anywhere.
async fn promote(
    request: &Request,
    daemon: &Arc<Daemon>,
    writer: &mut (impl AsyncWriteExt + Unpin + Send),
) -> Option<Value> {
    let from = request
        .params
        .get("from")
        .and_then(Value::as_str)
        .and_then(Tier::parse)
        .unwrap_or(Tier::Working);
    let to = request
        .params
        .get("to")
        .and_then(Value::as_str)
        .and_then(Tier::parse)
        .unwrap_or(match from {
            Tier::Working => Tier::Session,
            _ => Tier::LongTerm,
        });

    let entries = match daemon.memory.entries(from) {
        Ok(entries) => entries,
        Err(error) => {
            let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
            let _ = write_line(writer, &body).await;
            return None;
        }
    };
    if entries.is_empty() {
        return Some(json!({
            "promoted": false,
            "reason": format!("{} memory is empty, so there is nothing to promote", from.label()),
        }));
    }

    let joined: Vec<String> = entries
        .iter()
        .map(|entry| format!("- {}", entry.content))
        .collect();
    let instruction = match to {
        Tier::LongTerm => "Distil these notes from today into the few facts worth keeping \
                           permanently: preferences, decisions, and anything about how this \
                           person works. Drop anything that was only true for today.",
        _ => "Summarise these working notes into a short record of what was done and \
              decided. Keep names, paths and numbers exactly as written.",
    };
    let prompt = format!(
        "{}\n\nWrite plain prose, at most six lines, with no preamble.\n\n{}",
        instruction,
        joined.join("\n")
    );

    // Summarisation is local work by design.
    let local = daemon
        .router
        .read()
        .ok()
        .map(|router| router.config().local.clone())
        .unwrap_or_else(|| daemon.default_provider.clone());
    let provider = match daemon.registry.get(&local) {
        Some(provider) => provider,
        None => {
            let body = failure(
                request.id.clone(),
                INVALID_PARAMS,
                &format!("no local provider named `{}` to summarise with", local),
            );
            let _ = write_line(writer, &body).await;
            return None;
        }
    };

    let mut stream = match provider
        .complete(CompletionRequest {
            messages: vec![Message {
                role: "user".to_string(),
                content: prompt,
            }],
            // A stable key: the same kind of work every time, so the prefix
            // stays warm across promotions.
            cache_key: Some(format!("promote-{}-to-{}", from.label(), to.label())),
            ..Default::default()
        })
        .await
    {
        Ok(stream) => stream,
        Err(error) => {
            let body = failure(request.id.clone(), INTERNAL_ERROR, &error.to_string());
            let _ = write_line(writer, &body).await;
            return None;
        }
    };

    let mut summary = String::new();
    while let Some(token) = futures_util::StreamExt::next(&mut stream).await {
        match token {
            Ok(token) => summary.push_str(&token.text),
            Err(error) => {
                let body = failure(request.id.clone(), INTERNAL_ERROR, &error.to_string());
                let _ = write_line(writer, &body).await;
                return None;
            }
        }
    }

    let summary = summary.trim().to_string();
    if summary.is_empty() {
        let body = failure(
            request.id.clone(),
            INTERNAL_ERROR,
            "the local model returned an empty summary, so nothing was promoted",
        );
        let _ = write_line(writer, &body).await;
        return None;
    }

    match daemon.memory.promote(from, to, &summary) {
        Ok(id) => {
            info!(from = from.label(), to = to.label(), "promoted");
            Some(json!({
                "promoted": true,
                "id": id,
                "from": from.label(),
                "to": to.label(),
                "summarised": entries.len(),
                "summary": summary,
            }))
        }
        Err(error) => {
            let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
            let _ = write_line(writer, &body).await;
            None
        }
    }
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
