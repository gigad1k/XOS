//! The cloud supervisor tier.
//!
//! Wakes on events only, never continuously. Decomposes goals, re-plans
//! failures, compiles prompts for the local model and reviews batched work.
//!
//! Continuous is the thing to avoid. A supervisor that runs all the time costs
//! real money every month, breaks the offline guarantee because the machine
//! stops working without it, and widens egress from a few bounded digests to a
//! constant stream. So it sleeps, something wakes it, it thinks once, and it
//! sleeps again.
//!
//! # It only ever sees a digest
//!
//! [`Supervisor::wake`] takes a [`Digest`] and nothing else. It cannot be handed
//! raw memory or raw tool output, because there is no parameter to pass them in.
//! On top of that the digest is re-scanned immediately before it leaves, and a
//! wake carrying anything credential-shaped is refused rather than sent.

pub mod prompts;

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::policy::digest::Digest;
use crate::policy::redact;

pub use prompts::{CompiledPrompt, PromptCache};

/// Why the supervisor woke. Each carries a cost class, so an expensive trigger
/// can be throttled without silencing a cheap one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Trigger {
    /// A goal arrived and needs decomposing.
    NewGoal,
    /// A node exhausted its retries and needs re-planning.
    NodeFailure,
    /// Every N completed nodes, read the digest and course-correct.
    BatchReview,
    /// A new task type, or one failing too often.
    PromptCompile,
    /// The local model's own probabilities say it is guessing.
    ConfidenceFloor,
    /// Nothing in memory resembles this.
    NovelSituation,
    /// Scheduled planning and the morning brief.
    DailyPass,
    /// A person asked. Never throttled.
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CostClass {
    Low,
    Medium,
    High,
}

impl Trigger {
    pub fn label(&self) -> &'static str {
        match self {
            Trigger::NewGoal => "new-goal",
            Trigger::NodeFailure => "node-failure",
            Trigger::BatchReview => "batch-review",
            Trigger::PromptCompile => "prompt-compile",
            Trigger::ConfidenceFloor => "confidence-floor",
            Trigger::NovelSituation => "novel-situation",
            Trigger::DailyPass => "daily-pass",
            Trigger::Manual => "manual",
        }
    }

    pub fn cost_class(&self) -> CostClass {
        match self {
            Trigger::NewGoal | Trigger::NodeFailure => CostClass::High,
            Trigger::BatchReview | Trigger::PromptCompile | Trigger::DailyPass => CostClass::Medium,
            Trigger::ConfidenceFloor | Trigger::NovelSituation => CostClass::Low,
            // Manual is never throttled, so its class is only for the log.
            Trigger::Manual => CostClass::High,
        }
    }

    pub fn throttled(&self) -> bool {
        !matches!(self, Trigger::Manual)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorConfig {
    /// Which provider the supervisor uses. Without one it never wakes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Tokens a day across every trigger.
    #[serde(default = "default_daily_tokens")]
    pub daily_token_ceiling: u64,
    /// Shortest gap between two wakes of each class, in seconds.
    #[serde(default = "default_high_gap")]
    pub high_cost_gap_secs: u64,
    #[serde(default = "default_medium_gap")]
    pub medium_cost_gap_secs: u64,
    #[serde(default = "default_low_gap")]
    pub low_cost_gap_secs: u64,
    /// Recompile a prompt once its failure rate passes this.
    #[serde(default = "default_failure_threshold")]
    pub recompile_above_failure_rate: f64,
}

fn default_daily_tokens() -> u64 {
    200_000
}
fn default_high_gap() -> u64 {
    60
}
fn default_medium_gap() -> u64 {
    300
}
fn default_low_gap() -> u64 {
    900
}
fn default_failure_threshold() -> f64 {
    0.2
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            provider: None,
            daily_token_ceiling: default_daily_tokens(),
            high_cost_gap_secs: default_high_gap(),
            medium_cost_gap_secs: default_medium_gap(),
            low_cost_gap_secs: default_low_gap(),
            recompile_above_failure_rate: default_failure_threshold(),
        }
    }
}

/// What a wake produced, or why it did not happen.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum WakeOutcome {
    /// The supervisor is not configured, so the local tier carries on alone.
    NotConfigured,
    /// Refused before sending, because the digest was not clean.
    Refused { reason: String },
    /// Throttled, or the daily ceiling is spent. The local tier carries on.
    Deferred { reason: String },
    /// No connectivity. Queued for when it comes back.
    Queued { queued: usize, reason: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct QueuedWake {
    pub trigger: Trigger,
    pub digest_text: String,
    pub queued_at: i64,
}

struct Budget {
    /// Midnight UTC of the day the counts belong to.
    day: i64,
    tokens_spent: u64,
    last_wake: [i64; 3],
}

pub struct Supervisor {
    config: SupervisorConfig,
    budget: Mutex<Budget>,
    queue: Mutex<VecDeque<QueuedWake>>,
}

impl Supervisor {
    pub fn new(config: SupervisorConfig) -> Self {
        Self {
            config,
            budget: Mutex::new(Budget {
                day: today(),
                tokens_spent: 0,
                last_wake: [0; 3],
            }),
            queue: Mutex::new(VecDeque::new()),
        }
    }

    pub fn config(&self) -> &SupervisorConfig {
        &self.config
    }

    /// May the supervisor wake for this trigger right now?
    ///
    /// Separated from waking so a caller can ask without sending anything, and
    /// so the answer is always a reason rather than a bare no.
    pub fn may_wake(&self, trigger: Trigger) -> Result<(), WakeOutcome> {
        if self.config.provider.is_none() {
            return Err(WakeOutcome::NotConfigured);
        }
        if !trigger.throttled() {
            return Ok(());
        }

        let mut budget = match self.budget.lock() {
            Ok(budget) => budget,
            Err(_) => return Ok(()),
        };
        let now = unix_now();
        if budget.day != today() {
            budget.day = today();
            budget.tokens_spent = 0;
        }

        if budget.tokens_spent >= self.config.daily_token_ceiling {
            return Err(WakeOutcome::Deferred {
                reason: format!(
                    "the daily ceiling of {} tokens is spent, so work stays local until tomorrow",
                    self.config.daily_token_ceiling
                ),
            });
        }

        let index = class_index(trigger.cost_class());
        let gap = match trigger.cost_class() {
            CostClass::High => self.config.high_cost_gap_secs,
            CostClass::Medium => self.config.medium_cost_gap_secs,
            CostClass::Low => self.config.low_cost_gap_secs,
        } as i64;
        let since = now - budget.last_wake[index];
        if budget.last_wake[index] > 0 && since < gap {
            return Err(WakeOutcome::Deferred {
                reason: format!(
                    "{} triggers wake at most every {}s, and the last was {}s ago",
                    trigger.label(),
                    gap,
                    since
                ),
            });
        }
        Ok(())
    }

    /// Check the digest is safe to send.
    ///
    /// The builder already redacted it. This checks again on the way out,
    /// because the guarantee is worth more than the microseconds: a digest that
    /// reached here carrying a credential is a bug, and sending it anyway would
    /// make that bug permanent.
    ///
    /// This returns rather than panicking, so the refusal is testable and so a
    /// release build still refuses. The send path also carries a debug assertion
    /// on the same condition, to make the bug loud for whoever introduced it.
    pub fn vet(&self, digest: &Digest) -> Result<(), WakeOutcome> {
        let found = redact::scan(&digest.text);
        if !found.is_empty() {
            return Err(WakeOutcome::Refused {
                reason: format!(
                    "the digest still held {} credential-shaped strings, so nothing was sent",
                    found.len()
                ),
            });
        }
        if digest.estimated_tokens > crate::policy::digest::TOKEN_CAP {
            return Err(WakeOutcome::Refused {
                reason: format!(
                    "the digest is {} tokens, past the {} cap",
                    digest.estimated_tokens,
                    crate::policy::digest::TOKEN_CAP
                ),
            });
        }
        Ok(())
    }

    /// Record that a wake happened and what it cost.
    pub fn charge(&self, trigger: Trigger, tokens: u64) {
        if let Ok(mut budget) = self.budget.lock() {
            if budget.day != today() {
                budget.day = today();
                budget.tokens_spent = 0;
            }
            budget.tokens_spent += tokens;
            budget.last_wake[class_index(trigger.cost_class())] = unix_now();
        }
    }

    /// Hold a wake until connectivity returns.
    ///
    /// Offline is not a failure. Cached prompts keep working and known task
    /// types keep executing; only new thinking waits.
    pub fn queue(&self, trigger: Trigger, digest: &Digest, reason: &str) -> WakeOutcome {
        let mut queue = match self.queue.lock() {
            Ok(queue) => queue,
            Err(_) => {
                return WakeOutcome::Refused {
                    reason: "the queue is unavailable".to_string(),
                }
            }
        };
        queue.push_back(QueuedWake {
            trigger,
            digest_text: digest.text.clone(),
            queued_at: unix_now(),
        });
        WakeOutcome::Queued {
            queued: queue.len(),
            reason: reason.to_string(),
        }
    }

    pub fn queued(&self) -> Vec<QueuedWake> {
        self.queue
            .lock()
            .map(|queue| queue.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Take the next queued wake, for when connectivity returns.
    pub fn take_queued(&self) -> Option<QueuedWake> {
        self.queue.lock().ok()?.pop_front()
    }

    pub fn tokens_spent_today(&self) -> u64 {
        self.budget
            .lock()
            .map(|budget| {
                if budget.day == today() {
                    budget.tokens_spent
                } else {
                    0
                }
            })
            .unwrap_or(0)
    }

    /// Should this task type be recompiled?
    pub fn should_recompile(&self, prompt: Option<&CompiledPrompt>) -> bool {
        match prompt {
            None => true,
            Some(prompt) => {
                // Wait for enough executions that the rate means something.
                let total = prompt.hits + prompt.failures;
                total >= 10 && prompt.failure_rate() > self.config.recompile_above_failure_rate
            }
        }
    }
}

fn class_index(class: CostClass) -> usize {
    match class {
        CostClass::Low => 0,
        CostClass::Medium => 1,
        CostClass::High => 2,
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64
}

fn today() -> i64 {
    unix_now() / 86_400
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::digest::{self, Piece};

    fn supervisor(config: SupervisorConfig) -> Supervisor {
        Supervisor::new(SupervisorConfig {
            provider: Some("cloud".to_string()),
            ..config
        })
    }

    fn clean_digest() -> Digest {
        digest::build(
            &[Piece::new("memory:long-term", "the backup runs at 02:00")],
            digest::TOKEN_CAP,
        )
    }

    #[test]
    fn without_a_provider_nothing_wakes() {
        let supervisor = Supervisor::new(SupervisorConfig::default());
        assert_eq!(
            supervisor.may_wake(Trigger::NewGoal),
            Err(WakeOutcome::NotConfigured)
        );
    }

    #[test]
    fn a_first_wake_is_allowed() {
        let supervisor = supervisor(SupervisorConfig::default());
        assert!(supervisor.may_wake(Trigger::NewGoal).is_ok());
    }

    #[test]
    fn a_class_is_throttled_after_it_wakes() {
        let supervisor = supervisor(SupervisorConfig::default());
        supervisor.charge(Trigger::NewGoal, 100);

        let outcome = supervisor.may_wake(Trigger::NodeFailure);
        assert!(
            matches!(outcome, Err(WakeOutcome::Deferred { .. })),
            "a second high-cost wake should defer, got {:?}",
            outcome
        );
    }

    #[test]
    fn throttling_one_class_leaves_the_others_alone() {
        let supervisor = supervisor(SupervisorConfig::default());
        supervisor.charge(Trigger::NewGoal, 100);

        assert!(
            supervisor.may_wake(Trigger::ConfidenceFloor).is_ok(),
            "a cheap trigger must not be silenced by an expensive one"
        );
    }

    #[test]
    fn manual_is_never_throttled() {
        let supervisor = supervisor(SupervisorConfig::default());
        supervisor.charge(Trigger::NewGoal, 100);
        supervisor.charge(Trigger::Manual, 100);
        assert!(supervisor.may_wake(Trigger::Manual).is_ok());
    }

    #[test]
    fn the_daily_ceiling_degrades_to_local_rather_than_failing() {
        let supervisor = supervisor(SupervisorConfig {
            daily_token_ceiling: 1_000,
            ..Default::default()
        });
        supervisor.charge(Trigger::Manual, 1_500);

        match supervisor.may_wake(Trigger::NewGoal) {
            Err(WakeOutcome::Deferred { reason }) => {
                assert!(reason.contains("stays local"), "{}", reason);
            }
            other => panic!("expected a deferral, got {:?}", other),
        }
    }

    #[test]
    fn the_ceiling_counts_manual_wakes_too() {
        let supervisor = supervisor(SupervisorConfig {
            daily_token_ceiling: 100,
            ..Default::default()
        });
        supervisor.charge(Trigger::Manual, 500);
        assert_eq!(supervisor.tokens_spent_today(), 500);
    }

    #[test]
    fn a_clean_digest_passes_vetting() {
        let supervisor = supervisor(SupervisorConfig::default());
        assert!(supervisor.vet(&clean_digest()).is_ok());
    }

    #[test]
    fn a_digest_carrying_a_credential_is_refused() {
        // The builder redacts, so this can only happen through a bug. Refusing
        // is what stops a bug becoming a permanent leak.
        let supervisor = supervisor(SupervisorConfig::default());
        let dirty = Digest {
            text: "the key is sk-abcdefghijklmnop1234567890".to_string(),
            estimated_tokens: 10,
            dropped: 0,
            redactions: 0,
            sources: vec!["bug".to_string()],
        };
        match supervisor.vet(&dirty) {
            Err(WakeOutcome::Refused { reason }) => {
                assert!(reason.contains("nothing was sent"), "{}", reason)
            }
            other => panic!("expected a refusal, got {:?}", other),
        }
    }

    #[test]
    fn an_oversized_digest_is_refused() {
        let supervisor = supervisor(SupervisorConfig::default());
        let big = Digest {
            text: "safe text".to_string(),
            estimated_tokens: digest::TOKEN_CAP + 1,
            dropped: 0,
            redactions: 0,
            sources: Vec::new(),
        };
        assert!(matches!(
            supervisor.vet(&big),
            Err(WakeOutcome::Refused { .. })
        ));
    }

    #[test]
    fn offline_queues_rather_than_failing() {
        let supervisor = supervisor(SupervisorConfig::default());
        let outcome = supervisor.queue(Trigger::NewGoal, &clean_digest(), "no route to host");

        match outcome {
            WakeOutcome::Queued { queued, reason } => {
                assert_eq!(queued, 1);
                assert!(reason.contains("no route"));
            }
            other => panic!("expected queuing, got {:?}", other),
        }
        assert_eq!(supervisor.queued().len(), 1);
    }

    #[test]
    fn queued_wakes_come_back_in_order() {
        let supervisor = supervisor(SupervisorConfig::default());
        supervisor.queue(Trigger::NewGoal, &clean_digest(), "offline");
        supervisor.queue(Trigger::DailyPass, &clean_digest(), "offline");

        assert_eq!(
            supervisor.take_queued().expect("first").trigger,
            Trigger::NewGoal
        );
        assert_eq!(
            supervisor.take_queued().expect("second").trigger,
            Trigger::DailyPass
        );
        assert!(supervisor.take_queued().is_none());
    }

    #[test]
    fn a_task_type_with_no_prompt_is_compiled() {
        let supervisor = supervisor(SupervisorConfig::default());
        assert!(supervisor.should_recompile(None));
    }

    #[test]
    fn a_healthy_prompt_is_left_alone() {
        let supervisor = supervisor(SupervisorConfig::default());
        let prompt = CompiledPrompt {
            task_type: "tidy".to_string(),
            body: "x".to_string(),
            cache_key: "tidy-v1".to_string(),
            compiled_at: 0,
            version: 1,
            hits: 95,
            failures: 5,
            score: Some(0.9),
            guarded: true,
        };
        assert!(!supervisor.should_recompile(Some(&prompt)));
    }

    #[test]
    fn a_failing_prompt_is_recompiled() {
        let supervisor = supervisor(SupervisorConfig::default());
        let prompt = CompiledPrompt {
            task_type: "tidy".to_string(),
            body: "x".to_string(),
            cache_key: "tidy-v1".to_string(),
            compiled_at: 0,
            version: 1,
            hits: 50,
            failures: 50,
            score: Some(0.5),
            guarded: true,
        };
        assert!(supervisor.should_recompile(Some(&prompt)));
    }

    #[test]
    fn a_few_failures_are_not_yet_a_pattern() {
        // Recompiling on two failures out of three would thrash.
        let supervisor = supervisor(SupervisorConfig::default());
        let prompt = CompiledPrompt {
            task_type: "tidy".to_string(),
            body: "x".to_string(),
            cache_key: "tidy-v1".to_string(),
            compiled_at: 0,
            version: 1,
            hits: 1,
            failures: 2,
            score: Some(0.5),
            guarded: true,
        };
        assert!(!supervisor.should_recompile(Some(&prompt)));
    }

    #[test]
    fn every_trigger_has_a_cost_class() {
        let triggers = [
            Trigger::NewGoal,
            Trigger::NodeFailure,
            Trigger::BatchReview,
            Trigger::PromptCompile,
            Trigger::ConfidenceFloor,
            Trigger::NovelSituation,
            Trigger::DailyPass,
            Trigger::Manual,
        ];
        assert_eq!(triggers.len(), 8);
        assert_eq!(Trigger::NewGoal.cost_class(), CostClass::High);
        assert_eq!(Trigger::BatchReview.cost_class(), CostClass::Medium);
        assert_eq!(Trigger::NovelSituation.cost_class(), CostClass::Low);
    }
}
