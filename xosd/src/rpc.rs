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

use crate::graph::{Graph, NodeSpec, NodeState, SystemState};
use crate::journal::{Filter as JournalFilter, Journal};
use crate::memory::{bundle, Memory, Tier};
use crate::policy::log::PolicyLog;
use crate::scheduler::{EnergyLog, MachineState, PowerManager, Pulse, TickAction};
use crate::policy::{
    digest, redact, Context as PolicyContext, Permit, Policy, PolicyDecision,
};
use crate::providers::{CompletionRequest, Message, ProviderRegistry};
use crate::router::log::{EscalationLog, Record};
use crate::router::{self, CostMode, Observation, Router, Tier as RouteTier};
use crate::spend::{cost_of, SpendBook};
use crate::state::Halt;
use crate::supervisor::{Supervisor, Trigger, WakeOutcome};
use crate::supervisor::prompts::PromptCache;
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
    pub supervisor: Arc<Supervisor>,
    pub prompt_cache: Arc<PromptCache>,
    pub graph: Arc<Graph>,
    pub pulse: Arc<Pulse>,
    /// Where the local model is served from, so the GPU can be released.
    pub local_base_url: String,
    pub local_model: String,
    pub power: Arc<PowerManager>,
    pub energy: Arc<EnergyLog>,
    /// Tokens and elapsed time for a reply currently streaming, so the status
    /// bar can show a live rate. Cleared when the reply ends.
    pub live: RwLock<Option<(Instant, u32)>>,
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
        supervisor: Arc<Supervisor>,
        prompt_cache: Arc<PromptCache>,
        graph: Arc<Graph>,
        pulse: Arc<Pulse>,
        power: Arc<PowerManager>,
        energy: Arc<EnergyLog>,
        local_base_url: String,
        local_model: String,
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
            supervisor,
            prompt_cache,
            graph,
            pulse,
            power,
            energy,
            live: RwLock::new(None),
            local_base_url,
            local_model,
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    /// The rate of a reply streaming right now, if one is.
    fn live_rate(&self) -> Option<f64> {
        let live = self.live.read().ok()?;
        let (started, tokens) = (*live)?;
        let seconds = started.elapsed().as_secs_f64();
        if seconds < 0.2 || tokens == 0 {
            return None;
        }
        Some(tokens as f64 / seconds)
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

        // Hardware. Read-only in the strongest sense: this never installs,
        // loads or modifies anything. HW-2 owns installation, and keeping the
        // line here means detection can be run on a machine that is already
        // unhappy without making it worse.
        "hardware.inventory" => {
            let inventory = crate::hardware::Inventory::read();
            Some(json!({
                "inventory": inventory,
                "summary": inventory.summary(),
                "profile": inventory.profile().label(),
            }))
        }

        "hardware.resolve" => {
            match crate::hardware::Database::load() {
                Ok(database) => {
                    let resolve = |devices: &[crate::hardware::Device]| {
                        devices
                            .iter()
                            .map(|device| database.resolve(device))
                            .collect::<Vec<_>>()
                    };

                    // A device may be named rather than found. Someone planning
                    // an install, or helping a stranger over a chat window,
                    // needs to ask what XOS would do with a card that is not in
                    // this machine.
                    if let Some(named) = request
                        .params
                        .get("device")
                        .and_then(Value::as_str)
                    {
                        let class = request
                            .params
                            .get("class")
                            .and_then(Value::as_str)
                            .unwrap_or("display");
                        return match crate::hardware::Device::named(named, class) {
                            Some(device) => Some(json!({
                                "display": if class == "display" {
                                    vec![database.resolve(&device)]
                                } else {
                                    Vec::new()
                                },
                                "network": if class == "network" {
                                    vec![database.resolve(&device)]
                                } else {
                                    Vec::new()
                                },
                                "hypothetical": true,
                                "database_updated": database.updated,
                            })),
                            None => {
                                let body = failure(
                                    request.id.clone(),
                                    INVALID_PARAMS,
                                    "a device is written vendor:device in hex, as in 10de:1b80",
                                );
                                let _ = write_line(writer, &body).await;
                                None
                            }
                        };
                    }

                    let inventory = crate::hardware::Inventory::read();
                    Some(json!({
                        "display": resolve(&inventory.gpus),
                        "network": resolve(&inventory.network),
                        "profile": inventory.profile().label(),
                        "database_updated": database.updated,
                    }))
                }
                Err(error) => {
                    // Without the table there is nothing honest to say, and
                    // guessing a driver is how a machine ends up without a
                    // screen.
                    let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
                    let _ = write_line(writer, &body).await;
                    None
                }
            }
        }

        "hardware.profile" => {
            let inventory = crate::hardware::Inventory::read();
            let profile = inventory.profile();
            Some(json!({
                "profile": profile.label(),
                "vram_mb": inventory.gpus.iter().filter_map(|g| g.vram_mb).max(),
                "memory_mb": inventory.memory.total_mb,
                "cpu_level": inventory.cpu.microarchitecture_level,
                "baseline_ok": inventory.cpu.baseline_ok,
            }))
        }

        "pulse.status" => {
            let machine = MachineState::read();
            let today = daemon.energy.today().unwrap_or_else(|_| crate::scheduler::power::DayEnergy {
                day: "today".to_string(),
                watt_hours: 0.0,
                samples: 0,
            });
            let tariff = daemon.power.config().tariff_per_kwh;
            Some(json!({
                "halted": daemon.halt.is_halted(),
                "tick_secs": daemon.pulse.config().tick_secs,
                "idle_secs": daemon.power.idle_secs(),
                "tokens_per_second": daemon.live_rate(),
                "model_loaded": daemon.power.model_loaded(),
                "machine": machine,
                "summary": machine.summary(),
                "energy": {
                    "watt_hours_today": today.watt_hours,
                    "cost_today": today.cost(tariff),
                    "tariff_per_kwh": tariff,
                    "samples": today.samples,
                    "estimate": true,
                },
                "energy_recent": daemon.energy.recent(7).unwrap_or_default(),
                "spend_today": daemon.spend.spent_today_total().unwrap_or(0.0),
                // Reported, never run from a status call: suspending the
                // machine is not a side effect of asking how it is.
                "suspend_command": daemon.power.suspend_command(
                    daemon.pulse.config().tick_secs
                ),
            }))
        }

        "pulse.tasks" => Some(json!({
            "tasks": daemon.pulse.tasks(),
            "watchers": daemon.pulse.config().watchers,
        })),

        // Run one tick now, rather than waiting for the heartbeat.
        "pulse.tick" => {
            let report = daemon.pulse.tick(daemon.halt.is_halted(), daemon.power.idle_secs());
            let carried = carry_out(daemon, &report.actions).await;
            Some(json!({
                "ran": report.ran,
                "reason": report.reason,
                "actions": report.actions,
                "outcome": carried,
            }))
        }

        "graph.create_goal" => create_goal(request, daemon, writer).await,

        "graph.advance" => advance(request, daemon, writer).await,

        // A node that spent its retries is the supervisor's problem now.
        "graph.needs_replan" => match daemon.graph.failed_nodes() {
            Ok(nodes) => Some(json!({
                "failed": nodes.iter().map(|node| json!({
                    "goal_id": node.goal_id,
                    "node_id": node.id,
                    "title": node.title,
                    "attempts": node.attempts,
                    "failure": node.failure,
                    "conditions": node.conditions.iter().map(|c| c.label()).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })),
            Err(error) => {
                let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
                let _ = write_line(writer, &body).await;
                None
            }
        },

        "graph.list" => match daemon.graph.goals() {
            Ok(goals) => Some(json!({"goals": goals})),
            Err(error) => {
                let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
                let _ = write_line(writer, &body).await;
                None
            }
        },

        "graph.status" => {
            let goal_id = request
                .params
                .get("goal_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match (daemon.graph.goal(goal_id), daemon.graph.nodes(goal_id)) {
                (Some(goal), Ok(nodes)) => Some(json!({
                    "goal": goal,
                    "nodes": nodes,
                    "waiting_on": nodes.iter().flat_map(|node| {
                        node.conditions.iter().map(|c| c.label().to_string())
                    }).collect::<Vec<_>>(),
                })),
                _ => {
                    let body = failure(
                        request.id.clone(),
                        INVALID_PARAMS,
                        &format!("no goal called `{}`", goal_id),
                    );
                    let _ = write_line(writer, &body).await;
                    None
                }
            }
        }

        "graph.confirm" => {
            let node_id = request
                .params
                .get("node_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match daemon.graph.approve(node_id) {
                Ok(()) => Some(json!({"node_id": node_id, "state": "pending"})),
                Err(error) => {
                    let body = failure(request.id.clone(), INVALID_PARAMS, &error);
                    let _ = write_line(writer, &body).await;
                    None
                }
            }
        }

        "supervisor.wake" => wake(request, daemon, writer).await,

        "supervisor.compile" => compile(request, daemon, writer).await,

        // Process what was queued while offline.
        "supervisor.flush" => {
            let Some(name) = daemon.supervisor.config().provider.clone() else {
                return Some(
                    serde_json::to_value(WakeOutcome::NotConfigured).unwrap_or(Value::Null),
                );
            };
            let Some(provider) = daemon.registry.get(&name) else {
                return Some(json!({"processed": 0, "remaining": daemon.supervisor.queued().len()}));
            };

            let mut processed = 0usize;
            let mut replies = Vec::new();
            while let Some(queued) = daemon.supervisor.take_queued() {
                match run_once(&provider, &queued.digest_text, None).await {
                    Ok((text, tokens)) => {
                        daemon.supervisor.charge(queued.trigger, tokens);
                        processed += 1;
                        replies.push(json!({
                            "trigger": queued.trigger.label(),
                            "tokens": tokens,
                            "text": text.chars().take(200).collect::<String>(),
                        }));
                    }
                    Err(error) => {
                        // Still offline: put it back and stop, rather than
                        // burning the queue against a dead connection.
                        daemon.supervisor.queue(queued.trigger, &digest::Digest {
                            text: queued.digest_text.clone(),
                            estimated_tokens: 0,
                            dropped: 0,
                            redactions: 0,
                            sources: Vec::new(),
                        }, &error);
                        break;
                    }
                }
            }
            info!(processed, "the supervisor worked through its queue");
            Some(json!({
                "processed": processed,
                "remaining": daemon.supervisor.queued().len(),
                "replies": replies,
            }))
        }

        // Count how a cached prompt performed, which is what drives recompiling.
        "supervisor.prompt_outcome" => {
            let task_type = request
                .params
                .get("task_type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let succeeded = request
                .params
                .get("succeeded")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            match daemon.prompt_cache.record_use(task_type, succeeded) {
                Ok(()) => {
                    let prompt = daemon.prompt_cache.get(task_type);
                    Some(json!({
                        "task_type": task_type,
                        "failure_rate": prompt.as_ref().map(|p| p.failure_rate()),
                        "recompile_due": daemon.supervisor.should_recompile(prompt.as_ref()),
                    }))
                }
                Err(error) => {
                    let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
                    let _ = write_line(writer, &body).await;
                    None
                }
            }
        }

        "supervisor.prompts" => match daemon.prompt_cache.list() {
            Ok(prompts) => Some(json!({
                "prompts": prompts,
                "tokens_today": daemon.supervisor.tokens_spent_today(),
                "daily_ceiling": daemon.supervisor.config().daily_token_ceiling,
            })),
            Err(error) => {
                let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
                let _ = write_line(writer, &body).await;
                None
            }
        },

        "supervisor.log" => {
            let limit = request
                .params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20) as u32;
            match daemon.prompt_cache.history(limit) {
                Ok(entries) => Some(json!({
                    "entries": entries,
                    "queued": daemon.supervisor.queued(),
                    "tokens_today": daemon.supervisor.tokens_spent_today(),
                })),
                Err(error) => {
                    let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
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
                            if let Ok(mut live) = daemon.live.write() {
                                *live = Some((started, tokens));
                            }
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

    // The reply has ended, so there is no live rate any more.
    if let Ok(mut live) = daemon.live.write() {
        *live = None;
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

/// Declare a goal, and ask the supervisor to decompose it.
///
/// Decomposition is supervisor work by design: a 4B model is poor at
/// long-horizon planning, and a bad plan costs far more than the one call that
/// would have produced a good one.
async fn create_goal(
    request: &Request,
    daemon: &Arc<Daemon>,
    writer: &mut (impl AsyncWriteExt + Unpin + Send),
) -> Option<Value> {
    let description = request
        .params
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if description.is_empty() {
        let body = failure(request.id.clone(), INVALID_PARAMS, "a goal needs a description");
        let _ = write_line(writer, &body).await;
        return None;
    }

    let goal_id = match daemon.graph.create_goal(&description) {
        Ok(id) => id,
        Err(error) => {
            let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
            let _ = write_line(writer, &body).await;
            return None;
        }
    };

    // A caller may supply the plan, which is what a re-plan and the tests do.
    if let Some(specs) = request.params.get("nodes").and_then(|value| {
        serde_json::from_value::<Vec<NodeSpec>>(value.clone()).ok()
    }) {
        return match daemon.graph.plan(&goal_id, &specs) {
            Ok(count) => Some(json!({"goal_id": goal_id, "nodes": count, "planned_by": "caller"})),
            Err(error) => {
                let body = failure(request.id.clone(), INVALID_PARAMS, &error);
                let _ = write_line(writer, &body).await;
                None
            }
        };
    }

    let packet = digest::build(
        &[digest::Piece::new("goal", description.clone())],
        digest::TOKEN_CAP,
    );
    if let Err(outcome) = daemon.supervisor.may_wake(Trigger::NewGoal) {
        return Some(json!({
            "goal_id": goal_id,
            "planned": false,
            "supervisor": serde_json::to_value(outcome).unwrap_or(Value::Null),
        }));
    }
    if let Err(outcome) = daemon.supervisor.vet(&packet) {
        return Some(json!({
            "goal_id": goal_id,
            "planned": false,
            "supervisor": serde_json::to_value(outcome).unwrap_or(Value::Null),
        }));
    }

    let Some(name) = daemon.supervisor.config().provider.clone() else {
        return Some(json!({
            "goal_id": goal_id,
            "planned": false,
            "supervisor": serde_json::to_value(WakeOutcome::NotConfigured).unwrap_or(Value::Null),
        }));
    };
    let Some(provider) = daemon.registry.get(&name) else {
        return Some(json!({"goal_id": goal_id, "planned": false}));
    };

    let instruction = format!(
        "Break this goal into ordered steps for a small local model to carry out.\n\n\
         Goal: {}\n\n\
         Reply with JSON only: an array of objects with \"title\", \"detail\", and \
         \"after\", where after lists the titles this step waits on. No prose.",
        description
    );

    match run_once(&provider, &instruction, None).await {
        Ok((text, tokens)) => {
            daemon.supervisor.charge(Trigger::NewGoal, tokens);
            match parse_plan(&text) {
                Some(specs) if !specs.is_empty() => match daemon.graph.plan(&goal_id, &specs) {
                    Ok(count) => {
                        info!(goal = %goal_id, nodes = count, "a goal was decomposed");
                        Some(json!({
                            "goal_id": goal_id,
                            "planned": true,
                            "nodes": count,
                            "planned_by": "supervisor",
                            "tokens": tokens,
                        }))
                    }
                    Err(error) => {
                        let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
                        let _ = write_line(writer, &body).await;
                        None
                    }
                },
                _ => {
                    warn!(goal = %goal_id, "the supervisor did not return a usable plan");
                    Some(json!({
                        "goal_id": goal_id,
                        "planned": false,
                        "reason": "the supervisor did not return a usable plan",
                        "reply": text.chars().take(200).collect::<String>(),
                    }))
                }
            }
        }
        Err(error) => {
            // Offline: the goal exists and waits rather than being lost.
            let outcome = daemon.supervisor.queue(Trigger::NewGoal, &packet, &error);
            Some(json!({
                "goal_id": goal_id,
                "planned": false,
                "supervisor": serde_json::to_value(outcome).unwrap_or(Value::Null),
            }))
        }
    }
}

/// Pull a JSON array of node specs out of a model's reply.
fn parse_plan(text: &str) -> Option<Vec<NodeSpec>> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<Vec<NodeSpec>>(&text[start..=end]).ok()
}

/// Run whatever is eligible right now.
///
/// Execution is local work, and it goes through the policy engine and the
/// router like anything else. The graph module holds no provider handle, so
/// there is no way around either.
async fn advance(
    request: &Request,
    daemon: &Arc<Daemon>,
    writer: &mut (impl AsyncWriteExt + Unpin + Send),
) -> Option<Value> {
    if daemon.halt.guard().is_err() {
        let body = failure(request.id.clone(), HALTED, "the system is halted.");
        let _ = write_line(writer, &body).await;
        return None;
    }

    let limit = request
        .params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(1) as usize;

    let state = if request
        .params
        .get("ignore_conditions")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        SystemState::unrestricted()
    } else {
        SystemState::read()
    };

    let ran = run_eligible(daemon, limit, &state).await;
    if ran.is_empty() {
        return Some(json!({"ran": 0, "reason": "nothing is eligible to run"}));
    }
    Some(json!({"ran": ran.len(), "nodes": ran}))
}

/// Run whatever is eligible, and report what happened to each node.
///
/// Shared by the `graph.advance` call and the heartbeat, so a node started by
/// the scheduler goes through exactly the same policy and routing as one started
/// by a person.
pub async fn run_eligible(
    daemon: &Arc<Daemon>,
    limit: usize,
    state: &SystemState,
) -> Vec<Value> {
    let eligible = match daemon.graph.eligible(state) {
        Ok(nodes) => nodes,
        Err(error) => {
            warn!(%error, "the graph could not be read");
            return Vec::new();
        }
    };
    if eligible.is_empty() {
        return Vec::new();
    }

    let local = daemon
        .router
        .read()
        .ok()
        .map(|router| router.config().local.clone())
        .unwrap_or_else(|| daemon.default_provider.clone());
    let Some(provider) = daemon.registry.get(&local) else {
        warn!(provider = %local, "the local provider is not configured");
        return Vec::new();
    };

    let mut ran = Vec::new();
    for node in eligible.into_iter().take(limit.max(1)) {
        // Policy first, and judged on what the step actually does rather than on
        // the fact that it is a step. A node titled "delete the old release" has
        // to reach a person the same way the tool call would.
        let action = node
            .title
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join("_")
            .to_lowercase();
        let decision = daemon.policy.evaluate(
            &action,
            &json!({"title": node.title, "detail": node.detail}),
            &PolicyContext {
                local_only: daemon.cost_mode() == CostMode::AggressiveLocal,
                ..Default::default()
            },
        );
        let _ = daemon
            .policy_log
            .record(&action, &node.title, &decision, "graph");

        match &decision {
            PolicyDecision::Block { reason } => {
                let _ = daemon.graph.fail(&node.id, reason);
                ran.push(json!({"node": node.title, "outcome": "blocked", "reason": reason}));
                continue;
            }
            PolicyDecision::Prompt { reason } => {
                if node.confirmed {
                    // A person already said yes to this one. Spend the approval
                    // so it covers this attempt and not every future one.
                    let _ = daemon.graph.clear_confirmation(&node.id);
                } else {
                    let _ = daemon.graph.needs_user(&node.id, reason);
                    ran.push(
                        json!({"node": node.title, "outcome": "needs-user", "reason": reason}),
                    );
                    continue;
                }
            }
            _ => {}
        }

        if daemon.graph.start(&node.id).is_err() {
            continue;
        }

        // A compiled prompt for this kind of work carries its cache key, so the
        // prefix stays warm across nodes.
        let compiled = daemon.prompt_cache.get("execute-node");
        let cache_key = compiled.as_ref().map(|prompt| prompt.cache_key.clone());
        let preamble = compiled
            .as_ref()
            .map(|prompt| format!("{}\n\n", prompt.body))
            .unwrap_or_default();
        let prompt = format!(
            "{}You are carrying out one step of a larger goal. Do it, then say what \
             happened in one or two sentences.\n\nStep: {}\n{}",
            preamble, node.title, node.detail
        );

        match run_once(&provider, &prompt, cache_key).await {
            Ok((text, _)) => {
                let summary = text.chars().take(600).collect::<String>();
                let _ = daemon.graph.finish(&node.id, &summary);
                let _ = daemon.prompt_cache.record_use("execute-node", true);
                info!(node = %node.title, "a node finished");
                ran.push(json!({"node": node.title, "outcome": "done", "result": summary}));
            }
            Err(error) => {
                let outcome = daemon.graph.fail(&node.id, &error).unwrap_or(NodeState::Failed);
                let _ = daemon.prompt_cache.record_use("execute-node", false);
                warn!(node = %node.title, %error, "a node failed");
                ran.push(json!({
                    "node": node.title,
                    "outcome": outcome.label(),
                    "reason": error,
                }));
            }
        }
    }

    ran
}

/// Do what a tick decided. Pulse chooses; this carries it out, so every action
/// still goes through the policy engine and the router.
pub async fn carry_out(daemon: &Arc<Daemon>, actions: &[TickAction]) -> Vec<Value> {
    let mut done = Vec::new();
    for action in actions {
        match action {
            TickAction::AdvanceGraph { nodes } => {
                let state = crate::graph::SystemState::read();
                let ran = run_eligible(daemon, *nodes, &state).await;
                if !ran.is_empty() {
                    daemon.power.touch();
                }
                done.push(json!({"action": "advance-graph", "nodes": ran}));
            }
            TickAction::StartTask { name, goal } | TickAction::WatchFired { name, goal } => {
                match daemon.graph.create_goal(goal) {
                    Ok(goal_id) => {
                        info!(task = %name, goal = %goal_id, "the heartbeat started a task");
                        done.push(json!({"action": "start-task", "name": name, "goal_id": goal_id}));
                    }
                    Err(error) => {
                        warn!(task = %name, %error, "a scheduled task could not start");
                    }
                }
            }
            TickAction::UnloadModel { idle_secs } => {
                // Releasing the GPU is the difference between free and £150 a year.
                let target = daemon
                    .router
                    .read()
                    .ok()
                    .map(|router| router.config().local.clone())
                    .unwrap_or_else(|| daemon.default_provider.clone());
                let outcome = match daemon.registry.get(&target) {
                    Some(_) => match daemon.power.unload(&daemon.local_base_url, &daemon.local_model) {
                        Ok(()) => {
                            info!(idle_secs, "released the GPU after idling");
                            "unloaded"
                        }
                        Err(error) => {
                            debug!(%error, "the endpoint would not unload");
                            "refused"
                        }
                    },
                    None => "no provider",
                };
                done.push(json!({"action": "unload-model", "outcome": outcome}));
            }
        }
    }
    done
}

/// Build a digest from the pieces a caller offers.
///
/// The supervisor is only ever given one of these. There is no path that hands
/// it raw memory or raw tool output, because there is no parameter for it.
fn digest_from(request: &Request) -> digest::Digest {
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
    digest::build(&pieces, digest::TOKEN_CAP)
}

fn trigger_from(request: &Request) -> Trigger {
    match request
        .params
        .get("trigger")
        .and_then(Value::as_str)
        .unwrap_or("manual")
    {
        "new-goal" => Trigger::NewGoal,
        "node-failure" => Trigger::NodeFailure,
        "batch-review" => Trigger::BatchReview,
        "prompt-compile" => Trigger::PromptCompile,
        "confidence-floor" => Trigger::ConfidenceFloor,
        "novel-situation" => Trigger::NovelSituation,
        "daily-pass" => Trigger::DailyPass,
        _ => Trigger::Manual,
    }
}

/// Ask the supervisor tier to think once.
async fn wake(
    request: &Request,
    daemon: &Arc<Daemon>,
    writer: &mut (impl AsyncWriteExt + Unpin + Send),
) -> Option<Value> {
    if daemon.halt.guard().is_err() {
        let body = failure(request.id.clone(), HALTED, "the system is halted.");
        let _ = write_line(writer, &body).await;
        return None;
    }

    let trigger = trigger_from(request);
    let packet = digest_from(request);

    if let Err(outcome) = daemon.supervisor.may_wake(trigger) {
        return Some(serde_json::to_value(outcome).unwrap_or(Value::Null));
    }
    if let Err(outcome) = daemon.supervisor.vet(&packet) {
        warn!(trigger = trigger.label(), "a wake was refused before sending");
        return Some(serde_json::to_value(outcome).unwrap_or(Value::Null));
    }
    // The guardrail, made loud for whoever introduced the bug. `vet` has already
    // refused in a release build; this stops a developer walking past it.
    debug_assert!(
        redact::scan(&packet.text).is_empty(),
        "a digest reached the supervisor carrying credential-shaped strings"
    );

    let Some(name) = daemon.supervisor.config().provider.clone() else {
        return Some(serde_json::to_value(WakeOutcome::NotConfigured).unwrap_or(Value::Null));
    };
    let Some(provider) = daemon.registry.get(&name) else {
        return Some(
            serde_json::to_value(WakeOutcome::Deferred {
                reason: format!("`{}` is not a configured provider", name),
            })
            .unwrap_or(Value::Null),
        );
    };

    let instruction = request
        .params
        .get("instruction")
        .and_then(Value::as_str)
        .unwrap_or("Read this context and say what should happen next, briefly.");
    let prompt = format!("{}\n\n{}", instruction, packet.text);

    match run_once(&provider, &prompt, None).await {
        Ok((text, tokens)) => {
            daemon.supervisor.charge(trigger, tokens);
            info!(trigger = trigger.label(), tokens, "the supervisor woke");
            Some(json!({
                "outcome": "woke",
                "trigger": trigger.label(),
                "tokens": tokens,
                "text": text,
                "digest_tokens": packet.estimated_tokens,
                "digest_redactions": packet.redactions,
            }))
        }
        Err(error) => {
            // Offline is not a failure. Cached prompts keep working and known
            // task types keep executing; only new thinking waits.
            let outcome = daemon.supervisor.queue(trigger, &packet, &error);
            warn!(trigger = trigger.label(), %error, "the supervisor is offline, so the wake is queued");
            Some(serde_json::to_value(outcome).unwrap_or(Value::Null))
        }
    }
}

/// Compile a prompt for a task type, and adopt it only if it scores better.
async fn compile(
    request: &Request,
    daemon: &Arc<Daemon>,
    writer: &mut (impl AsyncWriteExt + Unpin + Send),
) -> Option<Value> {
    let task_type = request
        .params
        .get("task_type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if task_type.is_empty() {
        let body = failure(request.id.clone(), INVALID_PARAMS, "no task type given");
        let _ = write_line(writer, &body).await;
        return None;
    }

    let existing = daemon.prompt_cache.get(&task_type);
    let forced = request
        .params
        .get("force")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let supplied = request.params.get("body").is_some();

    // A prompt that is working is left alone: recompiling costs money and risks
    // drift, and the failure rate is what says it is worth doing.
    if !forced && !supplied && !daemon.supervisor.should_recompile(existing.as_ref()) {
        return Some(json!({
            "task_type": task_type,
            "outcome": "not-due",
            "adopted": false,
            "failure_rate": existing.as_ref().map(|p| p.failure_rate()),
            "reason": "the prompt in use is performing well enough to leave alone",
        }));
    }

    // A caller can supply a body directly; otherwise the supervisor writes one.
    let candidate = match request.params.get("body").and_then(Value::as_str) {
        Some(body) => body.to_string(),
        None => {
            let packet = digest_from(request);
            if let Err(outcome) = daemon.supervisor.may_wake(Trigger::PromptCompile) {
                return Some(serde_json::to_value(outcome).unwrap_or(Value::Null));
            }
            if let Err(outcome) = daemon.supervisor.vet(&packet) {
                return Some(serde_json::to_value(outcome).unwrap_or(Value::Null));
            }
            let Some(name) = daemon.supervisor.config().provider.clone() else {
                return Some(
                    serde_json::to_value(WakeOutcome::NotConfigured).unwrap_or(Value::Null),
                );
            };
            let Some(provider) = daemon.registry.get(&name) else {
                return Some(
                    serde_json::to_value(WakeOutcome::Deferred {
                        reason: format!("`{}` is not a configured provider", name),
                    })
                    .unwrap_or(Value::Null),
                );
            };
            let instruction = format!(
                "Write tight instructions for a small local model that has to do `{}` \
                 repeatedly. Include what to do, the exact shape of the output, and the \
                 mistakes to avoid. Write the instructions only, with no preamble.\n\n{}",
                task_type, packet.text
            );
            match run_once(&provider, &instruction, None).await {
                Ok((text, tokens)) => {
                    daemon.supervisor.charge(Trigger::PromptCompile, tokens);
                    text
                }
                Err(error) => {
                    let outcome = daemon.supervisor.queue(Trigger::PromptCompile, &packet, &error);
                    return Some(serde_json::to_value(outcome).unwrap_or(Value::Null));
                }
            }
        }
    };

    // Score the candidate, and the incumbent, on the same cases.
    let cases: Vec<(String, String)> = request
        .params
        .get("eval_cases")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some((
                        item.get("prompt").and_then(Value::as_str)?.to_string(),
                        item.get("expect_contains")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    let local = daemon
        .router
        .read()
        .ok()
        .map(|router| router.config().local.clone())
        .unwrap_or_else(|| daemon.default_provider.clone());

    let mut candidate_score = None;
    let mut incumbent_score = existing.as_ref().and_then(|prompt| prompt.score);
    if !cases.is_empty() {
        if let Some(provider) = daemon.registry.get(&local) {
            candidate_score = Some(score(&provider, &candidate, &cases).await);
            // Re-score the incumbent on the same cases, so the comparison is
            // like for like rather than against a figure from another day.
            if let Some(current) = &existing {
                incumbent_score = Some(score(&provider, &current.body, &cases).await);
            }
        }
    }

    // The guard compares against the incumbent's score as just measured on the
    // same cases, so the comparison is like for like rather than against a
    // figure from another day.
    if let (Some(_), Some(measured)) = (&existing, incumbent_score) {
        let _ = daemon.prompt_cache.set_score(&task_type, measured);
    }
    let decision = daemon
        .prompt_cache
        .adopt(&task_type, &candidate, candidate_score);

    match decision {
        Ok(decision) => {
            let stored = daemon.prompt_cache.get(&task_type);
            info!(
                task_type = %task_type,
                outcome = decision.label(),
                "a prompt was considered"
            );
            Some(json!({
                "task_type": task_type,
                "outcome": decision.label(),
                "adopted": decision.adopts(),
                "candidate_score": candidate_score,
                "incumbent_score": incumbent_score,
                "cases_run": cases.len(),
                "cache_key": stored.as_ref().map(|p| p.cache_key.clone()),
                "version": stored.as_ref().map(|p| p.version),
            }))
        }
        Err(error) => {
            let body = failure(request.id.clone(), INTERNAL_ERROR, &error);
            let _ = write_line(writer, &body).await;
            None
        }
    }
}

/// Run one prompt to completion and report the text and what it cost.
async fn run_once(
    provider: &Arc<dyn crate::providers::Provider>,
    prompt: &str,
    cache_key: Option<String>,
) -> Result<(String, u64), String> {
    let mut stream = provider
        .complete(CompletionRequest {
            messages: vec![Message {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
            cache_key,
            ..Default::default()
        })
        .await
        .map_err(|e| e.to_string())?;

    let mut text = String::new();
    let mut tokens = 0u64;
    while let Some(token) = futures_util::StreamExt::next(&mut stream).await {
        let token = token.map_err(|e| e.to_string())?;
        text.push_str(&token.text);
        if let Some(usage) = token.usage {
            tokens = (usage.input_tokens + usage.output_tokens) as u64;
        }
    }
    Ok((text.trim().to_string(), tokens))
}

/// Score a prompt on real cases, using the local model that will run it.
async fn score(
    provider: &Arc<dyn crate::providers::Provider>,
    body: &str,
    cases: &[(String, String)],
) -> f64 {
    let mut passed = 0usize;
    for (prompt, expected) in cases {
        let combined = format!("{}\n\n{}", body, prompt);
        if let Ok((text, _)) = run_once(provider, &combined, None).await {
            if expected.is_empty() || text.to_lowercase().contains(&expected.to_lowercase()) {
                passed += 1;
            }
        }
    }
    if cases.is_empty() {
        0.0
    } else {
        passed as f64 / cases.len() as f64
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
