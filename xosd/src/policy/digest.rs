//! Building the context packet that goes to the supervisor.
//!
//! Memory is local-only and is never transmitted wholesale. When the cloud tier
//! needs context it gets a digest: bounded, redacted, and assembled here rather
//! than handed raw recall.
//!
//! Three properties, in order of importance. It is *redacted*, so a credential
//! that slipped into a note does not leave with it. It is *bounded*, capped at
//! 8k tokens, so a long history cannot quietly become an expensive request. It
//! is *attributed*, so each piece says where it came from and the reply can be
//! judged against its sources.

use serde::Serialize;

use super::redact;

/// The cap the supervisor's context is held to.
pub const TOKEN_CAP: usize = 8192;

/// Four characters per token, the usual approximation.
const CHARS_PER_TOKEN: usize = 4;

/// One piece of context, with where it came from.
#[derive(Debug, Clone)]
pub struct Piece {
    pub source: String,
    pub content: String,
}

impl Piece {
    pub fn new(source: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Digest {
    pub text: String,
    pub estimated_tokens: usize,
    /// How many pieces were left out because the cap was reached.
    pub dropped: usize,
    /// How many credential-shaped strings were removed on the way.
    pub redactions: usize,
    pub sources: Vec<String>,
}

/// Assemble a digest, redacting as it goes and stopping at the cap.
///
/// Pieces are taken in the order given, so the caller decides what matters
/// most; what does not fit is reported rather than silently dropped.
pub fn build(pieces: &[Piece], token_cap: usize) -> Digest {
    let cap = token_cap.min(TOKEN_CAP).max(1);
    let budget = cap * CHARS_PER_TOKEN;

    let mut text = String::new();
    let mut sources = Vec::new();
    let mut redactions = 0usize;
    let mut dropped = 0usize;

    for piece in pieces {
        let (clean, spans) = redact::redact(&piece.content);
        redactions += spans.len();

        let block = format!("[{}]\n{}\n\n", piece.source, clean.trim());
        if text.len() + block.len() > budget {
            // Take a partial piece only if a useful amount fits.
            let room = budget.saturating_sub(text.len());
            if room > 200 {
                let header = format!("[{}]\n", piece.source);
                let available = room.saturating_sub(header.len() + 20);
                let cut = floor_char_boundary(&clean, available);
                text.push_str(&header);
                text.push_str(&clean[..cut]);
                text.push_str("\n[truncated]\n\n");
                sources.push(piece.source.clone());
            }
            dropped += 1;
            continue;
        }
        text.push_str(&block);
        sources.push(piece.source.clone());
    }

    let text = text.trim_end().to_string();
    Digest {
        estimated_tokens: text.len().div_ceil(CHARS_PER_TOKEN),
        dropped,
        redactions,
        sources,
        text,
    }
}

/// Cut at a character boundary, so a multi-byte character is never split.
fn floor_char_boundary(text: &str, index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    let mut cut = index;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_digest_carries_its_pieces_with_sources() {
        let digest = build(
            &[
                Piece::new("memory:long-term", "I prefer dark themes"),
                Piece::new("tool:disk_usage", "/home is 82% full"),
            ],
            TOKEN_CAP,
        );
        assert!(digest.text.contains("dark themes"));
        assert!(digest.text.contains("82%"));
        assert!(digest.text.contains("[memory:long-term]"));
        assert_eq!(digest.sources.len(), 2);
        assert_eq!(digest.dropped, 0);
    }

    #[test]
    fn a_credential_never_reaches_the_digest() {
        let digest = build(
            &[Piece::new(
                "tool:read_file",
                "config loaded\nAPI_KEY=sk-abcdefghijklmnop1234567890\ndone",
            )],
            TOKEN_CAP,
        );
        assert!(!digest.text.contains("sk-abcdef"), "{}", digest.text);
        assert!(digest.text.contains("[redacted:"), "{}", digest.text);
        assert_eq!(digest.redactions, 1);
        assert!(digest.text.contains("config loaded"), "context must survive");
    }

    #[test]
    fn the_cap_is_respected() {
        let long = "x".repeat(200_000);
        let digest = build(&[Piece::new("tool:cat", long)], TOKEN_CAP);
        assert!(
            digest.estimated_tokens <= TOKEN_CAP,
            "digest was {} tokens",
            digest.estimated_tokens
        );
    }

    #[test]
    fn a_smaller_cap_is_respected_too() {
        let long = "word ".repeat(10_000);
        let digest = build(&[Piece::new("tool:cat", long)], 100);
        assert!(digest.estimated_tokens <= 100, "{}", digest.estimated_tokens);
    }

    #[test]
    fn what_does_not_fit_is_reported_rather_than_hidden() {
        let big = "y".repeat(60_000);
        let digest = build(
            &[
                Piece::new("first", big.clone()),
                Piece::new("second", big),
                Piece::new("third", "short"),
            ],
            TOKEN_CAP,
        );
        assert!(digest.dropped > 0, "dropping must be visible");
    }

    #[test]
    fn truncation_marks_itself() {
        let long = "z".repeat(100_000);
        let digest = build(&[Piece::new("tool:cat", long)], TOKEN_CAP);
        assert!(digest.text.contains("[truncated]"), "a cut must be visible");
    }

    #[test]
    fn multibyte_text_is_never_split_mid_character() {
        // Every character is three bytes, so a naive cut would panic.
        let text = "日本語".repeat(20_000);
        let digest = build(&[Piece::new("tool:cat", text)], 500);
        assert!(digest.estimated_tokens <= 500);
        assert!(digest.text.is_char_boundary(digest.text.len()));
    }

    #[test]
    fn an_empty_digest_is_valid() {
        let digest = build(&[], TOKEN_CAP);
        assert!(digest.text.is_empty());
        assert_eq!(digest.estimated_tokens, 0);
        assert_eq!(digest.redactions, 0);
    }

    #[test]
    fn the_cap_can_never_be_raised_past_the_ceiling() {
        let long = "w".repeat(500_000);
        let digest = build(&[Piece::new("tool:cat", long)], 999_999);
        assert!(digest.estimated_tokens <= TOKEN_CAP);
    }
}
