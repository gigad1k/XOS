//! Routing between the local reflex model and the cloud supervisor.
//!
//! Decides which tier serves a request, escalates when the local model cannot
//! carry it, and records every local-to-API escalation with its trigger.
//!
//! Local is the default and the API is an escalation, never the other way
//! round. Every decision to leave the machine has a named trigger and lands in
//! the log, because an escalation costs money and sends context off the box, and
//! both of those are things you should be able to audit after the fact.
//!
//! Routing lives here and nowhere else. Providers know nothing about tiers.

pub mod log;

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Why a request left the local model. Evaluated in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Trigger {
    /// The local model emitted a tool call the runtime could not dispatch,
    /// twice. One malformed call is noise; two is a pattern.
    SchemaFailure,
    /// The kind of work is one the local model is not expected to carry.
    TaskClass,
    /// The prompt does not fit the local model's practical window.
    ContextOverflow,
    /// Generation stalled, or never started.
    Throughput,
    /// The request implies more chained tool calls than the local tier handles.
    ToolComplexity,
    /// The model's own token probabilities say it is guessing.
    LowConfidence,
    /// The configured cost mode asks for the better model.
    CostMode,
    /// A person asked. Never throttled, never overridden.
    Manual,
}

impl Trigger {
    pub fn label(&self) -> &'static str {
        match self {
            Trigger::SchemaFailure => "schema-failure",
            Trigger::TaskClass => "task-class",
            Trigger::ContextOverflow => "context-overflow",
            Trigger::Throughput => "throughput",
            Trigger::ToolComplexity => "tool-complexity",
            Trigger::LowConfidence => "low-confidence",
            Trigger::CostMode => "cost-mode",
            Trigger::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskClass {
    Chat,
    Summarise,
    Lookup,
    CodeGeneration,
    MultiStep,
}

impl TaskClass {
    pub fn label(&self) -> &'static str {
        match self {
            TaskClass::Chat => "chat",
            TaskClass::Summarise => "summarise",
            TaskClass::Lookup => "lookup",
            TaskClass::CodeGeneration => "code-generation",
            TaskClass::MultiStep => "multi-step",
        }
    }

    /// Chat, summarise and lookup stay local. Code generation and long
    /// multi-step work go to the API.
    fn belongs_on_api(&self) -> bool {
        matches!(self, TaskClass::CodeGeneration | TaskClass::MultiStep)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CostMode {
    /// Stay local unless the local tier genuinely cannot do it.
    AggressiveLocal,
    /// The default. Escalate on the triggers, nothing more.
    Balanced,
    /// Prefer the better model for anything that is not trivial chat.
    BestQuality,
}

impl CostMode {
    pub fn label(&self) -> &'static str {
        match self {
            CostMode::AggressiveLocal => "aggressive-local",
            CostMode::Balanced => "balanced",
            CostMode::BestQuality => "best-quality",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_lowercase().replace('_', "-").as_str() {
            "aggressive-local" | "local" => Some(CostMode::AggressiveLocal),
            "balanced" => Some(CostMode::Balanced),
            "best-quality" | "quality" | "best" => Some(CostMode::BestQuality),
            _ => None,
        }
    }
}

impl Default for CostMode {
    fn default() -> Self {
        CostMode::Balanced
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Local,
    Api,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    /// Provider name for the local reflex tier.
    #[serde(default = "default_local")]
    pub local: String,
    /// Provider name for the supervisor tier. Without one, nothing escalates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default)]
    pub cost_mode: CostMode,
    /// Escalate mid-stream below this generation rate.
    #[serde(default = "default_rate_floor")]
    pub tokens_per_second_floor: f64,
    /// Escalate if the first token has not arrived by now.
    #[serde(default = "default_first_token_secs")]
    pub first_token_timeout_secs: u64,
    /// Fraction of the local context window treated as usable.
    #[serde(default = "default_headroom")]
    pub context_headroom: f32,
    /// More implied chained tool calls than this escalates.
    #[serde(default = "default_tool_threshold")]
    pub tool_complexity_threshold: usize,
    /// Mean token logprob across a tool call below this escalates.
    #[serde(default = "default_min_logprob")]
    pub min_mean_logprob: f32,
    /// How many tokens must be seen before confidence is judged. One token's
    /// logprob is noise; this trigger is about a span, not a coin flip.
    #[serde(default = "default_logprob_samples")]
    pub min_logprob_samples: u32,
}

fn default_local() -> String {
    "local".to_string()
}
fn default_rate_floor() -> f64 {
    6.0
}
fn default_first_token_secs() -> u64 {
    20
}
fn default_headroom() -> f32 {
    0.8
}
fn default_tool_threshold() -> usize {
    3
}
fn default_min_logprob() -> f32 {
    -1.0
}
fn default_logprob_samples() -> u32 {
    8
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            local: default_local(),
            api: None,
            cost_mode: CostMode::default(),
            tokens_per_second_floor: default_rate_floor(),
            first_token_timeout_secs: default_first_token_secs(),
            context_headroom: default_headroom(),
            tool_complexity_threshold: default_tool_threshold(),
            min_mean_logprob: default_min_logprob(),
            min_logprob_samples: default_logprob_samples(),
        }
    }
}

/// What the router is asked to place.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub prompt: String,
    /// Set by the caller when it knows; inferred otherwise.
    pub task_class: Option<TaskClass>,
    pub tools_offered: usize,
    /// Tokens the prompt is expected to occupy.
    pub estimated_tokens: u32,
    /// How many local attempts already failed schema validation.
    pub schema_failures: u32,
    /// A person asked for the better model.
    pub manual: bool,
}

/// Where a request should go, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub tier: Tier,
    /// Present only when escalating.
    pub trigger: Option<Trigger>,
    pub task_class: TaskClass,
    pub reason: String,
}

impl Decision {
    pub fn escalated(&self) -> bool {
        self.tier == Tier::Api
    }
}

/// What a running completion looks like so far.
#[derive(Debug, Clone, Default)]
pub struct Observation {
    pub elapsed: Duration,
    pub tokens: u32,
    pub first_token_seen: bool,
    /// Mean token logprob across the tool-call span, from the model's own
    /// probabilities. Never a self-reported confidence: small models are badly
    /// calibrated when asked how sure they are.
    pub mean_logprob: Option<f32>,
    /// How many tokens the mean was taken over.
    pub logprob_samples: u32,
    /// The reply so far failed schema validation.
    pub schema_failed: bool,
}

#[derive(Clone)]
pub struct Router {
    config: RouterConfig,
    /// The local model's full context window, from its capabilities.
    local_context_window: u32,
}

impl Router {
    pub fn new(config: RouterConfig, local_context_window: u32) -> Self {
        Self {
            config,
            local_context_window,
        }
    }

    pub fn config(&self) -> &RouterConfig {
        &self.config
    }

    pub fn set_cost_mode(&mut self, mode: CostMode) {
        self.config.cost_mode = mode;
    }

    /// Tokens of local context treated as usable.
    fn usable_context(&self) -> u32 {
        (self.local_context_window as f32 * self.config.context_headroom) as u32
    }

    /// Decide before the request runs. Triggers are evaluated in the documented
    /// order, and the first one that fires wins, so the logged reason is the
    /// reason rather than one of several.
    pub fn route(&self, request: &Request) -> Decision {
        let class = request
            .task_class
            .unwrap_or_else(|| classify(&request.prompt));

        // Manual is checked first in practice because it is never throttled and
        // never overridden, even by aggressive-local.
        if request.manual {
            return self.escalate(Trigger::Manual, class, "a person asked for the API tier");
        }

        // Without an API provider there is nowhere to escalate to.
        if self.config.api.is_none() {
            return self.stay(class, "no API provider is configured");
        }

        // 1. Schema validation failure, on the second failure.
        if request.schema_failures >= 2 {
            return self.escalate(
                Trigger::SchemaFailure,
                class,
                &format!(
                    "the local model emitted {} unusable tool calls",
                    request.schema_failures
                ),
            );
        }

        // aggressive-local stays put for everything the local tier can attempt.
        // Only the triggers that mean "it cannot" survive below.
        let frugal = self.config.cost_mode == CostMode::AggressiveLocal;

        // 2. Task class.
        if class.belongs_on_api() && !frugal {
            return self.escalate(
                Trigger::TaskClass,
                class,
                &format!("{} is API-tier work", class.label()),
            );
        }

        // 3. Context overflow. Fires even when frugal: it is a cannot, not a
        // preference.
        let usable = self.usable_context();
        if request.estimated_tokens > usable {
            return self.escalate(
                Trigger::ContextOverflow,
                class,
                &format!(
                    "{} tokens is beyond the local window of {}",
                    request.estimated_tokens, usable
                ),
            );
        }

        // 5. Tool complexity. (4 is mid-stream and cannot be seen yet.)
        let chained = implied_tool_calls(&request.prompt, request.tools_offered);
        if chained > self.config.tool_complexity_threshold && !frugal {
            return self.escalate(
                Trigger::ToolComplexity,
                class,
                &format!("the request implies about {} chained tool calls", chained),
            );
        }

        // 7. Cost mode.
        if self.config.cost_mode == CostMode::BestQuality && class != TaskClass::Chat {
            return self.escalate(
                Trigger::CostMode,
                class,
                "best-quality mode prefers the API tier",
            );
        }

        self.stay(class, "the local model can carry this")
    }

    /// Reconsider a completion already in flight. Returns an escalation when
    /// one is warranted, and nothing when the local model is doing fine.
    pub fn reconsider(&self, class: TaskClass, observation: &Observation) -> Option<Decision> {
        self.config.api.as_ref()?;

        // 1. A malformed tool call, seen while running.
        if observation.schema_failed {
            return Some(self.escalate(
                Trigger::SchemaFailure,
                class,
                "the reply failed schema validation",
            ));
        }

        // 4. Timeout, or generation below the floor.
        if !observation.first_token_seen
            && observation.elapsed >= Duration::from_secs(self.config.first_token_timeout_secs)
        {
            return Some(self.escalate(
                Trigger::Throughput,
                class,
                &format!(
                    "no first token after {}s",
                    self.config.first_token_timeout_secs
                ),
            ));
        }
        if observation.first_token_seen && observation.tokens > 0 {
            let seconds = observation.elapsed.as_secs_f64();
            if seconds > 1.0 {
                let rate = observation.tokens as f64 / seconds;
                if rate < self.config.tokens_per_second_floor {
                    return Some(self.escalate(
                        Trigger::Throughput,
                        class,
                        &format!(
                            "{:.1} tok/s is below the floor of {:.1}",
                            rate, self.config.tokens_per_second_floor
                        ),
                    ));
                }
            }
        }

        // 6. Logprob confidence, once there is enough of a span for the mean
        // to mean anything. A single token's probability says nothing.
        if let Some(mean) = observation.mean_logprob {
            if observation.logprob_samples >= self.config.min_logprob_samples
                && mean < self.config.min_mean_logprob
            {
                return Some(self.escalate(
                    Trigger::LowConfidence,
                    class,
                    &format!(
                        "mean token logprob {:.2} is below {:.2}",
                        mean, self.config.min_mean_logprob
                    ),
                ));
            }
        }

        None
    }

    fn escalate(&self, trigger: Trigger, class: TaskClass, reason: &str) -> Decision {
        Decision {
            tier: Tier::Api,
            trigger: Some(trigger),
            task_class: class,
            reason: reason.to_string(),
        }
    }

    fn stay(&self, class: TaskClass, reason: &str) -> Decision {
        Decision {
            tier: Tier::Local,
            trigger: None,
            task_class: class,
            reason: reason.to_string(),
        }
    }
}

/// Infer what kind of work a prompt is, when the caller did not say.
///
/// Deliberately shallow. A wrong guess costs one escalation or one retry, and a
/// caller that knows better passes the class in.
pub fn classify(prompt: &str) -> TaskClass {
    let text = prompt.to_lowercase();

    let code_markers = [
        "write a function",
        "implement",
        "refactor",
        "write code",
        "fix the bug",
        "unit test",
        "```",
        "compile",
        "stack trace",
    ];
    if code_markers.iter().any(|marker| text.contains(marker)) {
        return TaskClass::CodeGeneration;
    }

    let step_markers = ["then ", "after that", "step by step", "and then", "finally,"];
    let steps = step_markers
        .iter()
        .filter(|marker| text.contains(*marker))
        .count();
    if steps >= 2 {
        return TaskClass::MultiStep;
    }

    if text.contains("summarise") || text.contains("summarize") || text.contains("tl;dr") {
        return TaskClass::Summarise;
    }
    if text.starts_with("what")
        || text.starts_with("who")
        || text.starts_with("when")
        || text.starts_with("where")
        || text.contains("look up")
    {
        return TaskClass::Lookup;
    }
    TaskClass::Chat
}

/// Roughly how many chained tool calls a request implies.
///
/// Counts the conjunctions that join actions, and treats a large tool surface as
/// evidence on its own. It only has to be right about "one call" versus "a
/// sequence".
pub fn implied_tool_calls(prompt: &str, tools_offered: usize) -> usize {
    if tools_offered == 0 {
        return 0;
    }
    let text = prompt.to_lowercase();
    let joins = [" then ", " and then ", " after that", " next, ", " finally", "; "];
    let chained = joins
        .iter()
        .map(|join| text.matches(join).count())
        .sum::<usize>();
    // One call for the request itself, plus one per join.
    let implied = 1 + chained;
    // A wide tool surface implies the model has to pick and combine.
    if tools_offered >= 8 {
        implied.max(3)
    } else {
        implied
    }
}

/// Four characters per token is the usual approximation, and good enough to
/// decide whether a prompt is anywhere near the window.
pub fn estimate_tokens(text: &str) -> u32 {
    ((text.chars().count() as f64) / 4.0).ceil() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn router(mode: CostMode) -> Router {
        Router::new(
            RouterConfig {
                api: Some("openrouter".to_string()),
                cost_mode: mode,
                ..Default::default()
            },
            8192,
        )
    }

    fn request(prompt: &str) -> Request {
        Request {
            prompt: prompt.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn plain_chat_stays_local() {
        let decision = router(CostMode::Balanced).route(&request("how are you today"));
        assert_eq!(decision.tier, Tier::Local);
        assert!(decision.trigger.is_none());
    }

    #[test]
    fn nothing_escalates_without_an_api_provider() {
        let router = Router::new(RouterConfig::default(), 8192);
        let decision = router.route(&Request {
            prompt: "implement a binary search".to_string(),
            schema_failures: 5,
            ..Default::default()
        });
        assert_eq!(decision.tier, Tier::Local);
        assert!(decision.reason.contains("no API provider"));
    }

    // -- trigger 1 --------------------------------------------------------

    #[test]
    fn one_schema_failure_retries_local_and_two_escalate() {
        let router = router(CostMode::Balanced);
        let once = router.route(&Request {
            schema_failures: 1,
            ..request("check the disk")
        });
        assert_eq!(once.tier, Tier::Local, "one failure retries locally");

        let twice = router.route(&Request {
            schema_failures: 2,
            ..request("check the disk")
        });
        assert_eq!(twice.trigger, Some(Trigger::SchemaFailure));
    }

    // -- trigger 2 --------------------------------------------------------

    #[test]
    fn code_generation_goes_to_the_api() {
        let decision = router(CostMode::Balanced).route(&request("implement a ring buffer"));
        assert_eq!(decision.trigger, Some(Trigger::TaskClass));
        assert_eq!(decision.task_class, TaskClass::CodeGeneration);
    }

    #[test]
    fn summarise_and_lookup_stay_local() {
        let router = router(CostMode::Balanced);
        assert_eq!(
            router.route(&request("summarise this article")).tier,
            Tier::Local
        );
        assert_eq!(
            router.route(&request("what is the capital of Peru")).tier,
            Tier::Local
        );
    }

    // -- trigger 3 --------------------------------------------------------

    #[test]
    fn a_prompt_beyond_the_local_window_escalates() {
        let decision = router(CostMode::Balanced).route(&Request {
            estimated_tokens: 7000, // beyond 80% of 8192
            ..request("chat about something")
        });
        assert_eq!(decision.trigger, Some(Trigger::ContextOverflow));
    }

    #[test]
    fn context_overflow_escalates_even_when_frugal() {
        let decision = router(CostMode::AggressiveLocal).route(&Request {
            estimated_tokens: 7000,
            ..request("chat about something")
        });
        assert_eq!(
            decision.trigger,
            Some(Trigger::ContextOverflow),
            "aggressive-local must still yield when the prompt cannot fit"
        );
    }

    // -- trigger 4 --------------------------------------------------------

    #[test]
    fn a_stalled_first_token_escalates() {
        let decision = router(CostMode::Balanced)
            .reconsider(
                TaskClass::Chat,
                &Observation {
                    elapsed: Duration::from_secs(25),
                    first_token_seen: false,
                    ..Default::default()
                },
            )
            .expect("a stall must escalate");
        assert_eq!(decision.trigger, Some(Trigger::Throughput));
    }

    #[test]
    fn generation_below_the_floor_escalates_mid_stream() {
        let decision = router(CostMode::Balanced)
            .reconsider(
                TaskClass::Chat,
                &Observation {
                    elapsed: Duration::from_secs(10),
                    tokens: 20, // 2 tok/s, floor is 6
                    first_token_seen: true,
                    ..Default::default()
                },
            )
            .expect("a slow stream must escalate");
        assert_eq!(decision.trigger, Some(Trigger::Throughput));
    }

    #[test]
    fn a_healthy_stream_is_left_alone() {
        let outcome = router(CostMode::Balanced).reconsider(
            TaskClass::Chat,
            &Observation {
                elapsed: Duration::from_secs(10),
                tokens: 400,
                first_token_seen: true,
                mean_logprob: Some(-0.2),
                logprob_samples: 400,
                ..Default::default()
            },
        );
        assert!(outcome.is_none());
    }

    // -- trigger 5 --------------------------------------------------------

    #[test]
    fn a_long_chain_of_tool_calls_escalates() {
        let decision = router(CostMode::Balanced).route(&Request {
            tools_offered: 4,
            ..request("find the file then read it then back it up then tell me")
        });
        assert_eq!(decision.trigger, Some(Trigger::ToolComplexity));
    }

    #[test]
    fn a_single_tool_call_stays_local() {
        let decision = router(CostMode::Balanced).route(&Request {
            tools_offered: 3,
            ..request("check how much disk space is left")
        });
        assert_eq!(decision.tier, Tier::Local);
    }

    // -- trigger 6 --------------------------------------------------------

    #[test]
    fn low_token_confidence_escalates() {
        let decision = router(CostMode::Balanced)
            .reconsider(
                TaskClass::Chat,
                &Observation {
                    elapsed: Duration::from_secs(1),
                    tokens: 50,
                    first_token_seen: true,
                    mean_logprob: Some(-2.5),
                    logprob_samples: 50,
                    ..Default::default()
                },
            )
            .expect("a guessing model must escalate");
        assert_eq!(decision.trigger, Some(Trigger::LowConfidence));
    }

    #[test]
    fn one_unlucky_token_does_not_escalate() {
        // Escalating on a single low-probability token would send work to the
        // API on the strength of one coin flip.
        let outcome = router(CostMode::Balanced).reconsider(
            TaskClass::Chat,
            &Observation {
                elapsed: Duration::from_millis(200),
                tokens: 1,
                first_token_seen: true,
                mean_logprob: Some(-4.0),
                logprob_samples: 1,
                ..Default::default()
            },
        );
        assert!(
            outcome.is_none(),
            "confidence must be judged over a span, not one token"
        );
    }

    // -- trigger 7 --------------------------------------------------------

    #[test]
    fn best_quality_prefers_the_api_for_real_work() {
        let decision = router(CostMode::BestQuality).route(&request("summarise this report"));
        assert_eq!(decision.trigger, Some(Trigger::CostMode));
    }

    #[test]
    fn best_quality_still_keeps_small_talk_local() {
        let decision = router(CostMode::BestQuality).route(&request("morning, how are you"));
        assert_eq!(decision.tier, Tier::Local);
    }

    #[test]
    fn aggressive_local_holds_the_line_on_preferences() {
        let decision =
            router(CostMode::AggressiveLocal).route(&request("implement a ring buffer"));
        assert_eq!(
            decision.tier,
            Tier::Local,
            "a preference must not escalate in aggressive-local"
        );
    }

    // -- trigger 8 --------------------------------------------------------

    #[test]
    fn manual_escalation_is_never_throttled() {
        let decision = router(CostMode::AggressiveLocal).route(&Request {
            manual: true,
            ..request("hello")
        });
        assert_eq!(decision.trigger, Some(Trigger::Manual));
    }

    // -- ordering ---------------------------------------------------------

    #[test]
    fn the_first_trigger_in_order_is_the_one_reported() {
        // Schema failure outranks task class, so the reason is the schema.
        let decision = router(CostMode::Balanced).route(&Request {
            schema_failures: 2,
            ..request("implement a ring buffer")
        });
        assert_eq!(decision.trigger, Some(Trigger::SchemaFailure));
    }

    // -- helpers ----------------------------------------------------------

    #[test]
    fn cost_modes_parse_from_what_a_person_would_type() {
        assert_eq!(CostMode::parse("balanced"), Some(CostMode::Balanced));
        assert_eq!(
            CostMode::parse("aggressive-local"),
            Some(CostMode::AggressiveLocal)
        );
        assert_eq!(CostMode::parse("best"), Some(CostMode::BestQuality));
        assert_eq!(CostMode::parse("nonsense"), None);
    }

    #[test]
    fn classification_picks_the_obvious_cases() {
        assert_eq!(classify("write a function that sorts"), TaskClass::CodeGeneration);
        assert_eq!(classify("summarise the meeting"), TaskClass::Summarise);
        assert_eq!(classify("what time is it"), TaskClass::Lookup);
        assert_eq!(classify("hello there"), TaskClass::Chat);
    }

    #[test]
    fn no_tools_means_no_implied_calls() {
        assert_eq!(implied_tool_calls("do this then that", 0), 0);
    }

    #[test]
    fn a_wide_tool_surface_implies_combining() {
        assert!(implied_tool_calls("sort out my machine", 12) >= 3);
    }
}
