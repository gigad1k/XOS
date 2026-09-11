//! Policy engine and egress protection.
//!
//! Classifies actions as reversible or irreversible, blocks reads of
//! designated-secret paths, and scans and redacts context bound for any API so
//! secrets never leave the machine.
//!
//! # Two axes, judged independently
//!
//! *Reversibility* asks whether the action can be undone. Deleting, sending,
//! buying, publishing and installing cannot, so they prompt. Reading,
//! searching, summarising and drafting can, so they run.
//!
//! *Egress* asks whether data leaves. This is the axis that reversibility
//! misses: reading `~/.ssh/id_rsa` is perfectly reversible in file terms, and
//! once that content reaches a cloud API the secret has left permanently. So
//! designated-secret paths are blocked outright however reversible the read is,
//! and everything else bound for an API is scanned and redacted on the way out.
//!
//! Nothing gets direct shell, filesystem or browser access. Every tool call
//! passes through `evaluate`, and a tool can only be run with a [`Permit`],
//! which only this module issues.

pub mod digest;
pub mod log;
pub mod redact;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use redact::Span;

/// What the engine decided about one tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "decision", rename_all = "kebab-case")]
pub enum PolicyDecision {
    /// Reversible and carries nothing sensitive. Run it.
    Allow,
    /// Irreversible. A person decides.
    Prompt { reason: String },
    /// Not allowed at all, whatever the user says next.
    Block { reason: String },
    /// Allowed, but this must be removed before it goes anywhere.
    Redact { spans: Vec<Span> },
}

impl PolicyDecision {
    pub fn label(&self) -> &'static str {
        match self {
            PolicyDecision::Allow => "allow",
            PolicyDecision::Prompt { .. } => "prompt",
            PolicyDecision::Block { .. } => "block",
            PolicyDecision::Redact { .. } => "redact",
        }
    }

    pub fn reason(&self) -> String {
        match self {
            PolicyDecision::Allow => "reversible, and nothing sensitive".to_string(),
            PolicyDecision::Prompt { reason } => reason.clone(),
            PolicyDecision::Block { reason } => reason.clone(),
            PolicyDecision::Redact { spans } => {
                format!("{} credential-shaped strings removed", spans.len())
            }
        }
    }

    pub fn runs(&self) -> bool {
        !matches!(self, PolicyDecision::Block { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Strictness {
    /// Prompt for anything that writes, not just anything irreversible.
    Strict,
    /// The rules as documented.
    Standard,
    /// Still blocks secret paths and still redacts. Only the prompting relaxes.
    Permissive,
}

impl Default for Strictness {
    fn default() -> Self {
        Strictness::Standard
    }
}

impl Strictness {
    pub fn label(&self) -> &'static str {
        match self {
            Strictness::Strict => "strict",
            Strictness::Standard => "standard",
            Strictness::Permissive => "permissive",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_lowercase().as_str() {
            "strict" => Some(Strictness::Strict),
            "standard" => Some(Strictness::Standard),
            "permissive" => Some(Strictness::Permissive),
            _ => None,
        }
    }

    /// What this setting means, in the words the wizard uses.
    pub fn describe(&self) -> &'static str {
        match self {
            Strictness::Strict => {
                "asks before anything that writes, not only before what cannot be undone"
            }
            Strictness::Standard => "asks before anything that cannot be undone",
            Strictness::Permissive => {
                "asks rarely. Secret paths are still blocked and egress is still \
                 redacted; only the prompting relaxes"
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfig {
    #[serde(default)]
    pub strictness: Strictness,
    /// Extra paths to treat as secret, beyond the built-in list.
    #[serde(default)]
    pub secret_paths: Vec<String>,
    /// Search stays on a local SearXNG in aggressive-local mode.
    #[serde(default = "default_searxng")]
    pub local_search_url: String,
}

fn default_searxng() -> String {
    "http://localhost:8888".to_string()
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            strictness: Strictness::default(),
            secret_paths: Vec::new(),
            local_search_url: default_searxng(),
        }
    }
}

/// Paths whose contents must never reach a model, local or otherwise.
///
/// Matched as fragments against the whole path, so `~/.ssh`, `/home/me/.ssh/`
/// and `../.ssh/id_rsa` are all caught.
const SECRET_FRAGMENTS: &[&str] = &[
    "/.ssh",
    "/.gnupg",
    "/.aws",
    "/.azure",
    "/.kube",
    "/.docker/config.json",
    "/.config/gcloud",
    "/.config/gh/hosts.yml",
    "/.password-store",
    "/.local/share/keyrings",
    "/.gnome2/keyrings",
    "/keychains/",
    "/.mozilla/firefox",
    "/.config/google-chrome",
    "/.config/chromium",
    "cookies.sqlite",
    "login data",
    "key4.db",
    "logins.json",
    "/.netrc",
    "/.pgpass",
    "/.npmrc",
    "/.pypirc",
    "/.git-credentials",
    // XOS's own store is not readable by a tool either.
    "/xos/vault.age",
    "/xos/vault-identity.txt",
];

/// Suffixes that are secret whatever directory they live in.
const SECRET_SUFFIXES: &[&str] = &[".pem", ".key", ".p12", ".pfx", ".keystore", ".jks"];

/// Verbs that cannot be undone.
const IRREVERSIBLE_VERBS: &[&str] = &[
    "delete", "remove", "rm", "destroy", "drop", "purge", "wipe", "format", "send", "email",
    "post", "publish", "tweet", "reply", "purchase", "buy", "pay", "order", "checkout", "install",
    "uninstall", "upgrade", "sudo", "exec", "shell", "run", "kill", "shutdown", "reboot",
    "overwrite", "truncate", "move", "rename", "chmod", "chown", "push", "merge", "deploy",
];

/// Verbs that only ever read.
const READING_VERBS: &[&str] = &[
    "read", "get", "list", "search", "find", "show", "status", "summarise", "summarize", "draft",
    "describe", "count", "check", "recall", "inspect", "view", "preview", "diff", "query",
];

/// Verbs that only ever write. Strict mode prompts for these too.
const WRITING_VERBS: &[&str] = &["write", "create", "save", "update", "set", "edit", "append"];

/// What the engine is told about the call it is judging.
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// True when the result is headed for a cloud API.
    pub api_bound: bool,
    /// True when the request originated somewhere untrusted, such as an inbound
    /// message. Tightens what is allowed without asking.
    pub untrusted_source: bool,
    /// True in aggressive-local mode, where search stays on the local instance.
    pub local_only: bool,
}

pub struct Policy {
    config: PolicyConfig,
}

impl Policy {
    pub fn new(config: PolicyConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &PolicyConfig {
        &self.config
    }

    /// Judge one tool call.
    ///
    /// Blocks win over prompts, and prompts over allows, so tightening a rule
    /// can never accidentally loosen the outcome.
    pub fn evaluate(&self, tool: &str, arguments: &Value, context: &Context) -> PolicyDecision {
        let name = tool.to_lowercase();

        // Axis 2 first: a secret path is blocked however reversible the read is.
        for path in paths_in(arguments) {
            if let Some(reason) = self.secret_path_reason(&path) {
                return PolicyDecision::Block { reason };
            }
        }

        // Search must stay local when the machine is in aggressive-local mode.
        if context.local_only && is_search(&name) && !self.is_local_search(arguments) {
            return PolicyDecision::Block {
                reason: format!(
                    "aggressive-local mode keeps search on the local instance at {}",
                    self.config.local_search_url
                ),
            };
        }

        // Axis 1: reversibility.
        if let Some(verb) = matched_verb(&name, IRREVERSIBLE_VERBS) {
            return PolicyDecision::Prompt {
                reason: format!("`{}` cannot be undone", verb),
            };
        }
        if self.config.strictness == Strictness::Strict {
            if let Some(verb) = matched_verb(&name, WRITING_VERBS) {
                return PolicyDecision::Prompt {
                    reason: format!("strict mode asks before `{}` changes anything", verb),
                };
            }
        }

        // An untrusted origin does not get to act unattended, only to read.
        if context.untrusted_source && !is_read_only(&name) {
            return PolicyDecision::Prompt {
                reason: "this came from an untrusted source, so it is not run unattended"
                    .to_string(),
            };
        }

        PolicyDecision::Allow
    }

    /// Judge a tool's result on its way out. This is the egress half.
    pub fn evaluate_result(&self, text: &str, context: &Context) -> PolicyDecision {
        if !context.api_bound {
            // Nothing leaves the machine, so nothing needs removing.
            return PolicyDecision::Allow;
        }
        let spans = redact::scan(text);
        if spans.is_empty() {
            PolicyDecision::Allow
        } else {
            PolicyDecision::Redact { spans }
        }
    }

    fn secret_path_reason(&self, path: &str) -> Option<String> {
        let lowered = path.to_lowercase().replace('\\', "/");

        for fragment in SECRET_FRAGMENTS {
            if lowered.contains(fragment) {
                return Some(format!(
                    "{} holds credentials, and reading it is not reversible once the content leaves",
                    fragment.trim_start_matches('/')
                ));
            }
        }
        for suffix in SECRET_SUFFIXES {
            if lowered.ends_with(suffix) {
                return Some(format!("{} files hold key material", suffix));
            }
        }
        // A .env file anywhere.
        if lowered.ends_with("/.env")
            || lowered == ".env"
            || lowered.contains("/.env.")
            || lowered.ends_with(".env")
        {
            return Some("`.env` files hold credentials".to_string());
        }
        for extra in &self.config.secret_paths {
            let extra = extra.to_lowercase();
            if !extra.is_empty() && lowered.contains(&extra) {
                return Some(format!("{} is configured as secret", extra));
            }
        }
        None
    }

    fn is_local_search(&self, arguments: &Value) -> bool {
        arguments
            .get("url")
            .and_then(Value::as_str)
            .map(|url| url.starts_with(&self.config.local_search_url))
            .unwrap_or(false)
    }
}

/// A tool may only run when the engine has judged it.
///
/// Holding one is proof a decision was made. No tool execution path may
/// construct a permit any other way, which is what keeps the engine from being
/// bypassed by accident.
#[derive(Debug, Clone)]
pub struct Permit {
    tool: String,
    decision: PolicyDecision,
}

// `decision` and `authorises` have no caller yet because XOS has no tool
// executor yet: tools arrive with the task graph. They are written now, and
// kept, because the guardrail is that no execution path may bypass the engine,
// and the gate has to exist before the thing it gates. The first executor calls
// `authorises` on every call it makes.
#[allow(dead_code)]
impl Permit {
    /// Issued only after evaluation, and only when the decision allows running.
    pub fn issue(tool: &str, decision: PolicyDecision) -> Option<Self> {
        if decision.runs() {
            Some(Self {
                tool: tool.to_string(),
                decision,
            })
        } else {
            None
        }
    }

    pub fn tool(&self) -> &str {
        &self.tool
    }

    pub fn decision(&self) -> &PolicyDecision {
        &self.decision
    }

    /// The assertion the guardrail asks for. Every executor calls this with the
    /// call it is about to make; a mismatch means a permit was reused for a
    /// different tool, which is a bug worth failing loudly for.
    pub fn authorises(&self, tool: &str) -> bool {
        let matches = self.tool == tool;
        debug_assert!(
            matches,
            "a permit for `{}` was used to run `{}`: every tool call must be judged on its own",
            self.tool, tool
        );
        matches
    }
}

/// Split a tool name into its words: `draft_reply` becomes `draft`, `reply`.
fn segments(name: &str) -> Vec<&str> {
    name.split(|c| c == '_' || c == '-' || c == '.')
        .filter(|piece| !piece.is_empty())
        .collect()
}

/// Which verb describes what a tool *does*.
///
/// The leading word is the action and the rest are usually its object, so
/// `draft_reply` drafts and `delete_file` deletes. Judging on any word would
/// make drafting a reply look like sending one, which would prompt for
/// something entirely reversible and teach people to click through prompts.
fn matched_verb(name: &str, verbs: &[&str]) -> Option<String> {
    let segments = segments(name);
    let Some(first) = segments.first() else {
        return None;
    };

    // A decisive leading verb settles it.
    if verbs.contains(first) {
        return Some((*first).to_string());
    }
    if READING_VERBS.contains(first) {
        // The action is a read; a noun later in the name does not change that.
        return None;
    }

    // No leading verb worth trusting, so any word counts.
    segments
        .iter()
        .find(|segment| verbs.contains(*segment))
        .map(|segment| (*segment).to_string())
}

fn is_read_only(name: &str) -> bool {
    let segments = segments(name);
    match segments.first() {
        Some(first) if READING_VERBS.contains(first) => true,
        Some(first) if IRREVERSIBLE_VERBS.contains(first) || WRITING_VERBS.contains(first) => false,
        _ => segments
            .iter()
            .any(|segment| READING_VERBS.contains(segment)),
    }
}

fn is_search(name: &str) -> bool {
    name.contains("search") || name.contains("web") || name.contains("browse")
}

/// Pull anything path-shaped out of a tool's arguments, at any depth.
fn paths_in(arguments: &Value) -> Vec<String> {
    let mut found = Vec::new();
    collect(arguments, &mut found);
    found
}

fn collect(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::String(text) => {
            // Anything that looks like a path at all is worth checking; the
            // cost of checking a string that was not one is nothing.
            if text.contains('/') || text.contains('\\') || text.starts_with('~') {
                found.push(text.clone());
            }
        }
        Value::Array(items) => items.iter().for_each(|item| collect(item, found)),
        Value::Object(map) => map.values().for_each(|item| collect(item, found)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy() -> Policy {
        Policy::new(PolicyConfig::default())
    }

    fn strict() -> Policy {
        Policy::new(PolicyConfig {
            strictness: Strictness::Strict,
            ..Default::default()
        })
    }

    // -- axis 2: secret paths --------------------------------------------

    #[test]
    fn reading_an_ssh_key_is_blocked() {
        let decision = policy().evaluate(
            "read_file",
            &json!({"path": "/home/shahr/.ssh/id_rsa"}),
            &Context::default(),
        );
        assert!(
            matches!(decision, PolicyDecision::Block { .. }),
            "got {:?}",
            decision
        );
        assert!(!decision.runs());
    }

    #[test]
    fn a_tilde_path_is_blocked_too() {
        let decision = policy().evaluate(
            "read_file",
            &json!({"path": "~/.ssh/id_ed25519"}),
            &Context::default(),
        );
        assert!(matches!(decision, PolicyDecision::Block { .. }));
    }

    #[test]
    fn every_designated_directory_is_blocked() {
        let paths = [
            "/home/me/.gnupg/secring.gpg",
            "/home/me/.aws/credentials",
            "/home/me/.config/gcloud/credentials.db",
            "/home/me/.kube/config",
            "/home/me/.password-store/site.gpg",
            "/home/me/.local/share/keyrings/login.keyring",
            "/home/me/.mozilla/firefox/abc/logins.json",
            "/home/me/.config/google-chrome/Default/Login Data",
            "/home/me/.netrc",
            "/home/me/.git-credentials",
        ];
        for path in paths {
            let decision = policy().evaluate("read_file", &json!({"path": path}), &Context::default());
            assert!(
                matches!(decision, PolicyDecision::Block { .. }),
                "{} should be blocked, got {:?}",
                path,
                decision
            );
        }
    }

    #[test]
    fn key_and_pem_files_are_blocked_anywhere() {
        for path in ["/srv/certs/server.pem", "/tmp/whatever.key", "/opt/a.p12"] {
            let decision = policy().evaluate("read_file", &json!({"path": path}), &Context::default());
            assert!(
                matches!(decision, PolicyDecision::Block { .. }),
                "{} should be blocked",
                path
            );
        }
    }

    #[test]
    fn a_dotenv_file_is_blocked() {
        let decision = policy().evaluate(
            "read_file",
            &json!({"path": "/home/me/project/.env"}),
            &Context::default(),
        );
        assert!(matches!(decision, PolicyDecision::Block { .. }));
    }

    #[test]
    fn xos_own_vault_is_not_readable_by_a_tool() {
        let decision = policy().evaluate(
            "read_file",
            &json!({"path": "/home/me/.local/share/xos/vault.age"}),
            &Context::default(),
        );
        assert!(matches!(decision, PolicyDecision::Block { .. }));
    }

    #[test]
    fn a_secret_path_nested_in_arguments_is_still_found() {
        let decision = policy().evaluate(
            "archive",
            &json!({"job": {"targets": ["/home/me/docs", "/home/me/.ssh/id_rsa"]}}),
            &Context::default(),
        );
        assert!(
            matches!(decision, PolicyDecision::Block { .. }),
            "a path buried in nested arguments must still be seen"
        );
    }

    #[test]
    fn an_ordinary_read_is_allowed() {
        let decision = policy().evaluate(
            "read_file",
            &json!({"path": "/home/me/notes.md"}),
            &Context::default(),
        );
        assert_eq!(decision, PolicyDecision::Allow);
    }

    #[test]
    fn extra_secret_paths_from_config_are_honoured() {
        let policy = Policy::new(PolicyConfig {
            secret_paths: vec!["/srv/private".to_string()],
            ..Default::default()
        });
        let decision = policy.evaluate(
            "read_file",
            &json!({"path": "/srv/private/notes.txt"}),
            &Context::default(),
        );
        assert!(matches!(decision, PolicyDecision::Block { .. }));
    }

    // -- axis 1: reversibility -------------------------------------------

    #[test]
    fn deleting_prompts() {
        let decision = policy().evaluate(
            "delete_file",
            &json!({"path": "/tmp/old.txt"}),
            &Context::default(),
        );
        assert!(matches!(decision, PolicyDecision::Prompt { .. }), "{:?}", decision);
        assert!(decision.runs(), "a prompt still runs once a person agrees");
    }

    #[test]
    fn sending_buying_publishing_and_installing_all_prompt() {
        for tool in [
            "send_email",
            "purchase_item",
            "publish_post",
            "install_package",
            "run_command",
            "sudo_exec",
        ] {
            let decision = policy().evaluate(tool, &json!({}), &Context::default());
            assert!(
                matches!(decision, PolicyDecision::Prompt { .. }),
                "{} should prompt, got {:?}",
                tool,
                decision
            );
        }
    }

    #[test]
    fn reversible_work_is_allowed() {
        for tool in ["read_file", "search_files", "summarise_text", "draft_reply", "list_processes"] {
            assert_eq!(
                policy().evaluate(tool, &json!({}), &Context::default()),
                PolicyDecision::Allow,
                "{} should be allowed",
                tool
            );
        }
    }

    #[test]
    fn a_verb_inside_another_word_does_not_trigger() {
        // `undeleteable_thing` is not a delete.
        assert_eq!(
            policy().evaluate("describe_item", &json!({}), &Context::default()),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn the_leading_verb_decides_not_a_noun_later_on() {
        // Drafting a reply is reversible; sending one is not. Prompting for the
        // draft would be wrong, and would train people to click through.
        assert_eq!(
            policy().evaluate("draft_reply", &json!({}), &Context::default()),
            PolicyDecision::Allow
        );
        assert!(matches!(
            policy().evaluate("send_reply", &json!({}), &Context::default()),
            PolicyDecision::Prompt { .. }
        ));

        // Same shape again: reading a record of deletions is not deleting.
        assert_eq!(
            policy().evaluate("list_deleted", &json!({}), &Context::default()),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn a_command_runner_still_prompts() {
        for tool in ["run_command", "exec_shell", "shell_exec"] {
            assert!(
                matches!(
                    policy().evaluate(tool, &json!({}), &Context::default()),
                    PolicyDecision::Prompt { .. }
                ),
                "{} must prompt",
                tool
            );
        }
    }

    #[test]
    fn strict_mode_prompts_before_writing_too() {
        let decision = strict().evaluate("write_file", &json!({"path": "/tmp/a"}), &Context::default());
        assert!(matches!(decision, PolicyDecision::Prompt { .. }), "{:?}", decision);
        // Standard mode allows the same write.
        assert_eq!(
            policy().evaluate("write_file", &json!({"path": "/tmp/a"}), &Context::default()),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn a_blocked_path_stays_blocked_in_permissive_mode() {
        let permissive = Policy::new(PolicyConfig {
            strictness: Strictness::Permissive,
            ..Default::default()
        });
        let decision = permissive.evaluate(
            "read_file",
            &json!({"path": "~/.ssh/id_rsa"}),
            &Context::default(),
        );
        assert!(
            matches!(decision, PolicyDecision::Block { .. }),
            "loosening prompts must never loosen the egress axis"
        );
    }

    // -- untrusted input --------------------------------------------------

    #[test]
    fn an_untrusted_source_cannot_act_unattended() {
        let context = Context {
            untrusted_source: true,
            ..Default::default()
        };
        let decision = policy().evaluate("write_file", &json!({"path": "/tmp/a"}), &context);
        assert!(matches!(decision, PolicyDecision::Prompt { .. }), "{:?}", decision);
    }

    #[test]
    fn an_untrusted_source_may_still_read() {
        let context = Context {
            untrusted_source: true,
            ..Default::default()
        };
        assert_eq!(
            policy().evaluate("read_file", &json!({"path": "/tmp/a"}), &context),
            PolicyDecision::Allow
        );
    }

    // -- local-only search -------------------------------------------------

    #[test]
    fn aggressive_local_keeps_search_on_the_local_instance() {
        let context = Context {
            local_only: true,
            ..Default::default()
        };
        let decision = policy().evaluate(
            "web_search",
            &json!({"url": "https://www.google.com/search?q=x"}),
            &context,
        );
        assert!(matches!(decision, PolicyDecision::Block { .. }), "{:?}", decision);

        let local = policy().evaluate(
            "web_search",
            &json!({"url": "http://localhost:8888/search?q=x"}),
            &context,
        );
        assert_eq!(local, PolicyDecision::Allow);
    }

    // -- egress ------------------------------------------------------------

    #[test]
    fn a_result_bound_for_an_api_is_redacted() {
        let context = Context {
            api_bound: true,
            ..Default::default()
        };
        let decision =
            policy().evaluate_result("the key is sk-abcdefghijklmnop1234567890", &context);
        match decision {
            PolicyDecision::Redact { spans } => assert_eq!(spans.len(), 1),
            other => panic!("expected redaction, got {:?}", other),
        }
    }

    #[test]
    fn a_result_staying_on_the_machine_is_untouched() {
        let decision = policy()
            .evaluate_result("the key is sk-abcdefghijklmnop1234567890", &Context::default());
        assert_eq!(
            decision,
            PolicyDecision::Allow,
            "nothing leaves, so nothing needs removing"
        );
    }

    #[test]
    fn clean_text_bound_for_an_api_passes() {
        let context = Context {
            api_bound: true,
            ..Default::default()
        };
        assert_eq!(
            policy().evaluate_result("the backup runs at 02:00", &context),
            PolicyDecision::Allow
        );
    }

    // -- permits -----------------------------------------------------------

    #[test]
    fn a_blocked_call_issues_no_permit() {
        let decision = PolicyDecision::Block {
            reason: "no".to_string(),
        };
        assert!(
            Permit::issue("read_file", decision).is_none(),
            "a blocked call must not be runnable"
        );
    }

    #[test]
    fn an_allowed_call_issues_a_permit_for_that_tool() {
        let permit = Permit::issue("read_file", PolicyDecision::Allow).expect("a permit");
        assert!(permit.authorises("read_file"));
        assert_eq!(permit.decision(), &PolicyDecision::Allow);
    }

    #[test]
    fn a_prompted_call_can_run_once_agreed() {
        let permit = Permit::issue(
            "delete_file",
            PolicyDecision::Prompt {
                reason: "cannot be undone".to_string(),
            },
        );
        assert!(permit.is_some());
    }
}
