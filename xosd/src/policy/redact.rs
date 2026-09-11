//! Scanning text for credential-shaped strings before it can leave the machine.
//!
//! This exists for the case nobody anticipated. A path rule stops XOS reading
//! `~/.ssh`, but it cannot stop a repository the agent legitimately reads from
//! containing a committed `.env`. Reversibility does not help either: reading a
//! file is reversible, and the secret leaving is not.
//!
//! So everything bound for an API is scanned here first, and anything that looks
//! like a credential is replaced with a marker naming what it was. The marker
//! matters: a model that sees `[redacted: aws-access-key-id]` can still reason
//! about the shape of the problem without holding the secret.
//!
//! The rules lean towards catching too much. A redacted string that was not a
//! secret costs a little context; a missed one is permanent.

use std::sync::OnceLock;

use regex::Regex;
use serde::Serialize;

/// A stretch of text that must not leave the machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    /// What it looked like, for the marker and the log.
    pub kind: String,
}

struct Rule {
    kind: &'static str,
    pattern: Regex,
}

fn rules() -> &'static Vec<Rule> {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let build = |kind: &'static str, pattern: &str| Rule {
            kind,
            // Every pattern here is a literal in this file, so a failure is a
            // programming error rather than anything a user can cause.
            pattern: Regex::new(pattern).expect("a policy pattern must compile"),
        };
        vec![
            // Private keys, whole block.
            build(
                "private-key",
                r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
            ),
            build("openai-key", r"sk-[A-Za-z0-9_\-]{16,}"),
            build("anthropic-key", r"sk-ant-[A-Za-z0-9_\-]{16,}"),
            build("github-token", r"gh[pousr]_[A-Za-z0-9]{16,}"),
            build("github-pat", r"github_pat_[A-Za-z0-9_]{20,}"),
            build("aws-access-key-id", r"A(?:KIA|SIA|ROA|IDA)[0-9A-Z]{16}"),
            build("google-api-key", r"AIza[0-9A-Za-z_\-]{35}"),
            build("slack-token", r"xox[baprs]-[A-Za-z0-9\-]{10,}"),
            build("stripe-key", r"[rs]k_(?:live|test)_[A-Za-z0-9]{16,}"),
            build("json-web-token", r"eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}"),
            build("bearer-token", r"(?i)\bbearer\s+[A-Za-z0-9._\-]{16,}"),
            // The .env case: a secret-sounding name assigned a long value.
            build(
                "assigned-secret",
                r#"(?i)\b[A-Z0-9_]*(?:API[_\-]?KEY|SECRET|TOKEN|PASSWORD|PASSWD|CREDENTIAL|PRIVATE[_\-]?KEY|ACCESS[_\-]?KEY)[A-Z0-9_]*\s*[:=]\s*["']?[^\s"'#]{8,}"#,
            ),
            // A postgres:// or similar URL carrying a password.
            build("connection-string", r"[a-zA-Z][a-zA-Z0-9+.\-]*://[^\s:/@]+:[^\s:/@]+@[^\s/]+"),
        ]
    })
}

/// Find everything credential-shaped in `text`.
///
/// Overlapping matches are merged, so a private key block inside a `.env` line
/// is redacted once rather than twice.
pub fn scan(text: &str) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    for rule in rules() {
        for found in rule.pattern.find_iter(text) {
            spans.push(Span {
                start: found.start(),
                end: found.end(),
                kind: rule.kind.to_string(),
            });
        }
    }
    merge(spans)
}

/// Overlapping spans become one, keeping the earliest and the longest reach.
fn merge(mut spans: Vec<Span>) -> Vec<Span> {
    if spans.is_empty() {
        return spans;
    }
    spans.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));

    let mut merged: Vec<Span> = Vec::new();
    for span in spans {
        match merged.last_mut() {
            Some(last) if span.start <= last.end => {
                if span.end > last.end {
                    last.end = span.end;
                    // Name both, so the log says what was actually in there.
                    if !last.kind.contains(&span.kind) {
                        last.kind = format!("{}+{}", last.kind, span.kind);
                    }
                }
            }
            _ => merged.push(span),
        }
    }
    merged
}

/// Replace every span with a marker naming what was there.
pub fn apply(text: &str, spans: &[Span]) -> String {
    if spans.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for span in spans {
        if span.start < cursor || span.end > text.len() {
            continue;
        }
        out.push_str(&text[cursor..span.start]);
        out.push_str(&format!("[redacted: {}]", span.kind));
        cursor = span.end;
    }
    out.push_str(&text[cursor.min(text.len())..]);
    out
}

/// Scan and replace in one step. This is what egress calls.
pub fn redact(text: &str) -> (String, Vec<Span>) {
    let spans = scan(text);
    (apply(text, &spans), spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redacted(text: &str) -> String {
        redact(text).0
    }

    #[test]
    fn an_openai_key_is_caught() {
        let out = redacted("use sk-abcdefghijklmnop1234567890 for the call");
        assert!(!out.contains("sk-abcdefghij"), "{}", out);
        assert!(out.contains("[redacted:"), "{}", out);
    }

    #[test]
    fn an_anthropic_key_is_caught() {
        let out = redacted("ANTHROPIC=sk-ant-api03-abcdefghijklmnopqrstuvwxyz");
        assert!(!out.contains("sk-ant-api03"), "{}", out);
    }

    #[test]
    fn a_github_token_is_caught() {
        let out = redacted("token ghp_1234567890abcdefghijABCDEFGH");
        assert!(!out.contains("ghp_1234567890"), "{}", out);
    }

    #[test]
    fn an_aws_key_id_is_caught() {
        let out = redacted("AKIAIOSFODNN7EXAMPLE is the id");
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"), "{}", out);
        assert!(out.contains("aws-access-key-id"), "{}", out);
    }

    #[test]
    fn a_private_key_block_is_caught_whole() {
        let text = "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\nsecretmaterial\n-----END RSA PRIVATE KEY-----\nafter";
        let out = redacted(text);
        assert!(!out.contains("secretmaterial"), "{}", out);
        assert!(!out.contains("MIIEowIBAAKCAQEA"), "{}", out);
        assert!(out.contains("before") && out.contains("after"));
    }

    #[test]
    fn the_committed_dotenv_case_is_caught() {
        // The case the path rules cannot reach: a repo XOS may legitimately read.
        let text = "DATABASE_URL=postgres://app:hunter2@db.internal:5432/app\nAPI_KEY=9f8a7b6c5d4e3f2a1b0c\nDEBUG=true";
        let out = redacted(text);
        assert!(!out.contains("9f8a7b6c5d4e3f2a1b0c"), "{}", out);
        assert!(!out.contains("hunter2"), "{}", out);
        assert!(out.contains("DEBUG=true"), "harmless lines must survive: {}", out);
    }

    #[test]
    fn a_jwt_is_caught() {
        let out = redacted(
            "auth eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N",
        );
        assert!(!out.contains("dozjgNryP4J3"), "{}", out);
    }

    #[test]
    fn a_bearer_header_is_caught() {
        let out = redacted("Authorization: Bearer abcdef1234567890abcdef");
        assert!(!out.contains("abcdef1234567890abcdef"), "{}", out);
    }

    #[test]
    fn ordinary_prose_is_left_alone() {
        let text = "The backup job runs at 02:00 and writes to /home/shahr/backups.";
        assert_eq!(redacted(text), text);
    }

    #[test]
    fn code_without_secrets_is_left_alone() {
        let text = "fn main() { println!(\"hello\"); }\nlet total = 42;";
        assert_eq!(redacted(text), text);
    }

    #[test]
    fn the_marker_says_what_was_there() {
        let out = redacted("key: sk-abcdefghijklmnop1234567890");
        assert!(out.contains("openai-key"), "{}", out);
    }

    #[test]
    fn several_secrets_in_one_blob_are_all_caught() {
        let text = "sk-aaaaaaaaaaaaaaaaaaaaaa and ghp_bbbbbbbbbbbbbbbbbbbb and AKIAIOSFODNN7EXAMPLE";
        let out = redacted(text);
        assert!(!out.contains("sk-aaaa"), "{}", out);
        assert!(!out.contains("ghp_bbbb"), "{}", out);
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"), "{}", out);
        assert_eq!(out.matches("[redacted:").count(), 3, "{}", out);
    }

    #[test]
    fn overlapping_matches_are_merged_not_doubled() {
        // Both the assigned-secret rule and the key rule match this line.
        let text = "OPENAI_API_KEY=sk-abcdefghijklmnop1234567890";
        let spans = scan(text);
        assert_eq!(spans.len(), 1, "overlaps must merge: {:?}", spans);
        let out = apply(text, &spans);
        assert_eq!(out.matches("[redacted:").count(), 1, "{}", out);
        assert!(!out.contains("sk-abcdef"), "{}", out);
    }

    #[test]
    fn redaction_does_not_lose_surrounding_text() {
        let text = "start sk-abcdefghijklmnop1234567890 middle ghp_bbbbbbbbbbbbbbbbbbbb end";
        let out = redacted(text);
        assert!(out.starts_with("start "), "{}", out);
        assert!(out.contains(" middle "), "{}", out);
        assert!(out.ends_with(" end"), "{}", out);
    }

    #[test]
    fn scanning_empty_text_is_safe() {
        assert_eq!(redacted(""), "");
        assert!(scan("").is_empty());
    }

    #[test]
    fn a_password_in_a_connection_string_is_caught() {
        let out = redacted("postgres://user:s3cr3tpass@host:5432/db");
        assert!(!out.contains("s3cr3tpass"), "{}", out);
    }
}
