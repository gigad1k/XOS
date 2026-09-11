//! Looking at the obvious places, read-only.
//!
//! This lists directories. It does not open a single file, and it never will:
//! the guardrail on the first goal is that it is read-only and risk-free, and
//! reading somebody's documents five minutes after they installed an operating
//! system would be neither.
//!
//! Even the listing goes through the policy engine, because a directory
//! somebody has marked as secret should be invisible to the first goal exactly
//! as it is to everything else. There is no exemption for being the first
//! thing XOS does.

use serde::Serialize;
use serde_json::json;

use crate::policy::{Context, Policy, PolicyDecision};

/// The obvious places, and only these.
///
/// Fixed rather than discovered. Walking a home directory looking for anything
/// interesting is a different and much less welcome act than looking in four
/// named places somebody can read in one line.
pub const DIRECTORIES: [&str; 4] = ["~/Documents", "~/Projects", "~/code", "~/Downloads"];

/// How many names to take from a directory. Enough to tell what somebody works
/// on, not enough to be an index of their disk.
const SAMPLE: usize = 24;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Scan {
    pub machine: String,
    pub profile: String,
    pub findings: Vec<Finding>,
    pub summary: String,
    pub memory_id: Option<i64>,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub directory: String,
    pub entries: usize,
    /// A few names, to say what kind of work this is.
    pub sample: Vec<String>,
    /// File extensions, most common first.
    pub kinds: Vec<String>,
}

/// List what is there.
///
/// A directory that is missing, unreadable or blocked by policy is skipped
/// without comment in the summary. Somebody whose home does not look like the
/// assumed shape should not have their first minute with XOS be a list of
/// complaints about it.
pub fn look(policy: &Policy, skipped: &mut Vec<String>) -> Vec<Finding> {
    let home = match dirs::home_dir() {
        Some(home) => home,
        None => return Vec::new(),
    };
    let context = Context {
        api_bound: false,
        untrusted_source: false,
        local_only: true,
    };

    let mut findings = Vec::new();
    for shorthand in DIRECTORIES {
        let path = home.join(shorthand.trim_start_matches("~/"));

        // Policy first, before the directory is even looked at.
        let decision = policy.evaluate(
            "list_directory",
            &json!({ "path": path.to_string_lossy() }),
            &context,
        );
        if matches!(decision, PolicyDecision::Block { .. }) {
            skipped.push(format!("{} (the firewall keeps it private)", shorthand));
            continue;
        }

        let Ok(entries) = std::fs::read_dir(&path) else {
            // Missing or unreadable. Silently, as the guardrail says.
            continue;
        };

        let mut names = Vec::new();
        let mut extensions: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut total = 0usize;

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // A dotfile is somebody's configuration, not their work.
            if name.starts_with('.') {
                continue;
            }
            total += 1;
            if names.len() < SAMPLE {
                names.push(name.clone());
            }
            if let Some((_, extension)) = name.rsplit_once('.') {
                if extension.len() <= 5 && extension.chars().all(|c| c.is_ascii_alphanumeric()) {
                    *extensions.entry(extension.to_lowercase()).or_insert(0) += 1;
                }
            }
        }

        if total == 0 {
            continue;
        }

        let mut kinds: Vec<(String, usize)> = extensions.into_iter().collect();
        kinds.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

        findings.push(Finding {
            directory: shorthand.to_string(),
            entries: total,
            sample: names,
            kinds: kinds.into_iter().take(6).map(|(kind, _)| kind).collect(),
        });
    }

    findings
}

impl Scan {
    /// The sentence that goes into memory.
    ///
    /// Written as something a person would recognise about themselves, because
    /// it is going to be recalled and shown back to them. A list of directory
    /// counts would be accurate and useless.
    pub fn compose(&self) -> String {
        let mut parts = Vec::new();
        parts.push(format!("This machine: {}.", self.machine));
        parts.push(format!(
            "XOS runs here in {} mode.",
            match self.profile.as_str() {
                "local" => "local".to_string(),
                "cpu" => "CPU-only".to_string(),
                other => other.to_string(),
            }
        ));

        if self.findings.is_empty() {
            parts.push(
                "Nothing in the usual project directories yet, so there is \
                 nothing to say about what they work on."
                    .to_string(),
            );
            return parts.join(" ");
        }

        for finding in &self.findings {
            let kinds = if finding.kinds.is_empty() {
                String::new()
            } else {
                format!(", mostly {}", finding.kinds.join(", "))
            };
            parts.push(format!(
                "{} holds {} things{}.",
                finding.directory, finding.entries, kinds
            ));
        }

        if let Some(guess) = self.guess() {
            parts.push(guess);
        }

        parts.join(" ")
    }

    /// One careful sentence about what this looks like.
    ///
    /// Hedged on purpose. This is a guess from filenames, it will sometimes be
    /// wrong, and a confident wrong statement about somebody is a worse first
    /// impression than no statement at all.
    fn guess(&self) -> Option<String> {
        let mut kinds: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for finding in &self.findings {
            for kind in &finding.kinds {
                *kinds.entry(kind.as_str()).or_insert(0) += 1;
            }
        }

        let code = ["rs", "py", "js", "ts", "go", "c", "cpp", "java", "rb", "sh"];
        let writing = ["md", "txt", "doc", "docx", "odt", "pdf", "tex"];
        let media = ["png", "jpg", "jpeg", "mp4", "mov", "wav", "mp3", "svg"];

        let count = |group: &[&str]| group.iter().filter(|k| kinds.contains_key(*k)).count();
        let (coding, written, made) = (count(&code), count(&writing), count(&media));

        if coding == 0 && written == 0 && made == 0 {
            return None;
        }
        let leading = if coding >= written && coding >= made {
            "writing software"
        } else if written >= made {
            "writing"
        } else {
            "working with images, audio or video"
        };
        Some(format!(
            "On the evidence of filenames alone, they appear to spend their time \
             {}. That is a guess from names, not from anything opened, and it may \
             well be wrong.",
            leading
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PolicyConfig;

    fn scan_with(findings: Vec<Finding>) -> Scan {
        Scan {
            machine: "a test machine".to_string(),
            profile: "local".to_string(),
            findings,
            summary: String::new(),
            memory_id: None,
            skipped: Vec::new(),
        }
    }

    fn finding(directory: &str, entries: usize, kinds: &[&str]) -> Finding {
        Finding {
            directory: directory.to_string(),
            entries,
            sample: Vec::new(),
            kinds: kinds.iter().map(|k| k.to_string()).collect(),
        }
    }

    #[test]
    fn nothing_is_opened_only_listed() {
        // The guardrail, as a test over the source of this module: a first goal
        // that read somebody's documents would be a very different thing from
        // the one described to them.
        let source = include_str!("scan.rs");
        let body = source
            .split("#[cfg(test)]")
            .next()
            .expect("the code above the tests");
        for forbidden in ["read_to_string", "File::open", "fs::read("] {
            assert!(
                !body.contains(forbidden),
                "`{}` would open a file, and this goal only lists",
                forbidden
            );
        }
    }

    #[test]
    fn only_the_four_named_places_are_looked_at() {
        // Walking a home directory for anything interesting is a different and
        // much less welcome act than looking in four places somebody can read
        // in one line.
        assert_eq!(DIRECTORIES.len(), 4);
        assert!(DIRECTORIES.iter().all(|d| d.starts_with("~/")));
    }

    #[test]
    fn a_missing_directory_is_skipped_without_complaint() {
        let policy = Policy::new(PolicyConfig::default());
        let mut skipped = Vec::new();
        // Whatever is on this machine, nothing may panic and nothing absent may
        // end up in the list of things to tell somebody about.
        let findings = look(&policy, &mut skipped);
        for finding in &findings {
            assert!(finding.entries > 0, "an empty directory is not a finding");
        }
        for note in &skipped {
            assert!(
                note.contains("firewall"),
                "only a policy block is worth mentioning, not an absence: {}",
                note
            );
        }
    }

    #[test]
    fn an_empty_home_produces_a_summary_that_says_so() {
        let summary = scan_with(Vec::new()).compose();
        assert!(summary.contains("This machine:"));
        assert!(
            summary.contains("nothing to say about what they work on"),
            "{}",
            summary
        );
    }

    #[test]
    fn a_coding_machine_reads_as_one() {
        let summary = scan_with(vec![
            finding("~/Projects", 14, &["rs", "toml"]),
            finding("~/code", 9, &["py", "js"]),
        ])
        .compose();
        assert!(summary.contains("writing software"), "{}", summary);
    }

    #[test]
    fn the_guess_is_always_marked_as_a_guess() {
        // It is inferred from filenames and will sometimes be wrong. A confident
        // wrong statement about somebody is a worse first impression than none.
        let summary = scan_with(vec![finding("~/Documents", 30, &["md", "pdf"])]).compose();
        assert!(summary.contains("may"), "{}", summary);
        assert!(
            summary.contains("guess") || summary.contains("appear"),
            "{}",
            summary
        );
    }

    #[test]
    fn the_summary_never_names_a_single_file() {
        // Samples are collected for the report Mission Control shows, but what
        // goes into long-term memory is about the shape of the work, not a list
        // of somebody's filenames.
        let mut scan = scan_with(vec![finding("~/Documents", 3, &["pdf"])]);
        scan.findings[0].sample = vec![
            "tax return 2024.pdf".to_string(),
            "divorce settlement.pdf".to_string(),
        ];
        let summary = scan.compose();
        assert!(!summary.contains("tax return"), "{}", summary);
        assert!(!summary.contains("divorce"), "{}", summary);
    }

    #[test]
    fn dotfiles_are_not_somebodys_work() {
        let policy = Policy::new(PolicyConfig::default());
        let mut skipped = Vec::new();
        for finding in look(&policy, &mut skipped) {
            for name in &finding.sample {
                assert!(!name.starts_with('.'), "{} is configuration", name);
            }
        }
    }
}
