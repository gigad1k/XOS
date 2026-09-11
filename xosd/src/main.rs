//! XOS Core — the intelligence layer daemon.
//!
//! Every other component is a thin client over this daemon's API. Nothing else
//! imports a model provider, MCP server or channel gateway directly.

mod config;
mod graph;
mod journal;
mod memory;
mod policy;
mod providers;
mod router;
mod rpc;
mod scheduler;
mod spend;
mod state;
mod supervisor;
mod tools;
mod vault;

use std::process::ExitCode;
use std::sync::Arc;

use tokio::net::UnixListener;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use config::{Config, ProviderConfig};
use providers::anthropic::AnthropicProvider;
use providers::llama_cpp::LlamaCppProvider;
use providers::openai::OpenAiCompatibleProvider;
use providers::ProviderRegistry;
use journal::Journal;
use memory::Memory;
use policy::log::PolicyLog;
use supervisor::{PromptCache, Supervisor};
use policy::Policy;
use router::log::EscalationLog;
use router::Router;
use rpc::Daemon;
use spend::SpendBook;
use state::Halt;
use vault::Vault;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("XOS_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error!(%error, "xosd stopped");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let config_path = config::config_path();
    let existed = config_path.exists();
    let config = Config::load_or_create(&config_path)
        .map_err(|e| format!("cannot read {}: {}", config_path.display(), e))?;
    if existed {
        info!(path = %config_path.display(), "config loaded");
    } else {
        info!(path = %config_path.display(), "config written with defaults");
    }

    let halt = Arc::new(Halt::load(config::halt_state_path()));
    if halt.is_halted() {
        warn!(
            state = %halt.state_path().display(),
            "starting halted. Autonomous work stays stopped until `xos resume`."
        );
    }

    let vault = Arc::new(Vault::open(&config::vault_dir()));
    info!(store = vault.backend().label(), "vault opened");

    let spend = Arc::new(
        SpendBook::open(&config::spend_path(), config.caps.clone())
            .map_err(|e| format!("spend: {}", e))?,
    );
    if let Some(cap) = config.caps.daily_total {
        info!(daily_total = cap, "a global daily spend cap is set");
    }

    let mut registry = ProviderRegistry::new();
    for (name, provider) in &config.providers {
        match provider {
            ProviderConfig::LlamaCpp(settings) => {
                let provider = LlamaCppProvider::new(name, settings.clone())
                    .map_err(|e| format!("provider `{}`: {}", name, e))?;
                info!(
                    provider = %name,
                    model = %settings.model,
                    prefix_cache = settings.slots > 0,
                    "local provider registered"
                );
                registry.insert(Arc::new(provider));
            }
            ProviderConfig::OpenAiCompatible(settings) => {
                let provider =
                    OpenAiCompatibleProvider::new(name, settings.clone(), Arc::clone(&vault))
                        .map_err(|e| format!("provider `{}`: {}", name, e))?;
                info!(provider = %name, model = %settings.model, "cloud provider registered");
                registry.insert(Arc::new(provider));
            }
            ProviderConfig::Anthropic(settings) => {
                let provider = AnthropicProvider::new(name, settings.clone(), Arc::clone(&vault))
                    .map_err(|e| format!("provider `{}`: {}", name, e))?;
                info!(provider = %name, model = %settings.model, "cloud provider registered");
                registry.insert(Arc::new(provider));
            }
        }
    }
    if registry.is_empty() {
        warn!("no providers are configured; add one to the config file");
    }
    if registry.get(&config.default_provider).is_none() {
        warn!(
            default = %config.default_provider,
            "the default provider is not configured; requests must name one"
        );
    }

    let (socket_path, moved) = config.resolve_socket();
    if let Some(reason) = moved {
        warn!("{}", reason);
    }
    if let Some(parent) = socket_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // A socket file left by a crashed daemon would block the bind.
    if socket_path.exists() {
        if tokio::net::UnixStream::connect(&socket_path).await.is_ok() {
            return Err(format!(
                "another xosd is listening on {}",
                socket_path.display()
            ));
        }
        std::fs::remove_file(&socket_path)
            .map_err(|e| format!("cannot clear {}: {}", socket_path.display(), e))?;
    }

    let listener = UnixListener::bind(&socket_path)
        .map_err(|e| format!("cannot listen on {}: {}", socket_path.display(), e))?;
    info!(socket = %socket_path.display(), "xosd listening");

    // The router needs the local model's window to judge context overflow.
    let local_context_window = registry
        .get(&config.router.local)
        .map(|provider| provider.capabilities().context_window)
        .unwrap_or(8192);

    let mut router_config = config.router.clone();
    router_config.cost_mode = config::effective_mode(config.router.clone());
    if router_config.api.is_none() {
        info!("no API tier is configured, so nothing will escalate");
    } else {
        info!(
            local = %router_config.local,
            api = router_config.api.as_deref().unwrap_or("none"),
            mode = router_config.cost_mode.label(),
            "router ready"
        );
    }
    let router = Router::new(router_config, local_context_window);

    let memory = Arc::new(
        Memory::open(&config::memory_path(), config.embedder.clone())
            .map_err(|e| format!("memory: {}", e))?,
    );
    info!(embedder = %memory.embedder().label(), "memory opened");

    let journal = Arc::new(
        Journal::open(
            &config::journal_path(),
            config::journal_store(),
            config.journal.clone(),
        )
        .map_err(|e| format!("journal: {}", e))?,
    );
    match journal.prune() {
        Ok(removed) if removed > 0 => info!(removed, "pruned journal entries past retention"),
        Ok(_) => {}
        Err(error) => warn!(%error, "the journal could not be pruned"),
    }
    info!(
        retain_days = journal.config().retain_days,
        "action journal ready"
    );

    let supervisor = Arc::new(Supervisor::new(config.supervisor.clone()));
    let prompt_cache = Arc::new(
        PromptCache::open(&config::prompts_path()).map_err(|e| format!("prompts: {}", e))?,
    );
    match &config.supervisor.provider {
        Some(provider) => info!(
            provider = %provider,
            daily_tokens = config.supervisor.daily_token_ceiling,
            "supervisor ready, and asleep"
        ),
        None => info!("no supervisor provider is configured, so XOS runs local-only"),
    }

    let policy = Arc::new(Policy::new(config.policy.clone()));
    let policy_log = Arc::new(
        PolicyLog::open(&config::policy_log_path()).map_err(|e| format!("policy log: {}", e))?,
    );
    info!(
        strictness = config.policy.strictness.label(),
        "policy engine ready"
    );

    let escalations = Arc::new(
        EscalationLog::open(&config::escalations_path())
            .map_err(|e| format!("escalation log: {}", e))?,
    );

    let daemon = Arc::new(Daemon::new(
        registry,
        Arc::clone(&halt),
        config.default_provider.clone(),
        vault,
        spend,
        router,
        escalations,
        Arc::clone(&memory),
        policy,
        policy_log,
        journal,
        supervisor,
        prompt_cache,
    ));

    if config.export.enabled {
        spawn_scheduled_export(config.clone(), Arc::clone(&memory));
    }

    tokio::select! {
        _ = rpc::serve(listener, daemon) => {}
        _ = shutdown() => {
            info!("xosd stopping");
        }
    }

    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}

/// Write an encrypted bundle on a timer, when asked to in config.
///
/// A dead disk should cost you nothing, but only if the export actually runs,
/// so this says out loud when it cannot: a missing path or passphrase is
/// reported once at startup rather than silently doing nothing for months.
fn spawn_scheduled_export(config: Config, memory: Arc<Memory>) {
    let Some(path) = config.export.path.clone() else {
        warn!("scheduled export is enabled but no path is set, so nothing will be written");
        return;
    };
    if std::env::var(&config.export.passphrase_env).is_err() {
        warn!(
            variable = %config.export.passphrase_env,
            "scheduled export is enabled but the passphrase variable is unset, so nothing will be written"
        );
        return;
    }

    let hours = config.export.every_hours.max(1);
    info!(path = %path.display(), every_hours = hours, "scheduled export is on");

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(hours * 3600));
        // The first tick fires immediately; skip it so startup is not a write.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let passphrase = match std::env::var(&config.export.passphrase_env) {
                Ok(passphrase) => passphrase,
                Err(_) => continue,
            };
            let entries = match memory.all() {
                Ok(entries) => entries,
                Err(error) => {
                    warn!(%error, "scheduled export could not read memory");
                    continue;
                }
            };
            let bundle = memory::bundle::Bundle::new(
                entries,
                serde_json::to_value(&config).unwrap_or(serde_json::Value::Null),
                Vec::new(),
            );
            match memory::bundle::export(&path, &bundle, &passphrase) {
                Ok(()) => info!(path = %path.display(), "exported"),
                Err(error) => warn!(%error, "scheduled export failed"),
            }
        }
    });
}

/// Stop on Ctrl-C, or on SIGTERM when the service manager asks.
async fn shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
