//! Turning text into a vector for recall.
//!
//! Two backends, chosen by config.
//!
//! `lexical` is the default: a hashed character-trigram vector computed in
//! process. It is small, runs on the CPU in microseconds, adds no dependency and
//! never competes with the model for the GPU, which is what the memory system
//! needs. Be honest about what it is: it matches wording, not meaning. It finds
//! "the ssh key path" from "ssh key", and it will not find it from "the thing I
//! use to log into the server".
//!
//! `remote` points at an OpenAI-compatible `/embeddings` endpoint for real
//! semantic vectors. Configure one when recall quality matters more than having
//! no moving parts.

use serde::{Deserialize, Serialize};

/// Dimensions of the lexical vector. Large enough that unrelated trigrams
/// rarely collide, small enough that a row costs a kilobyte.
pub const LEXICAL_DIMENSIONS: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EmbedderConfig {
    /// Hashed trigrams, in process.
    Lexical,
    /// An OpenAI-compatible embeddings endpoint.
    Remote {
        base_url: String,
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credential: Option<String>,
    },
}

impl Default for EmbedderConfig {
    fn default() -> Self {
        EmbedderConfig::Lexical
    }
}

impl EmbedderConfig {
    pub fn label(&self) -> String {
        match self {
            EmbedderConfig::Lexical => "lexical (in process, wording not meaning)".to_string(),
            EmbedderConfig::Remote { model, .. } => format!("remote ({})", model),
        }
    }
}

/// A hashed character-trigram vector, L2 normalised.
///
/// Trigrams rather than words so that "backup", "backups" and "backed up" share
/// most of their weight, and so a typo costs a little similarity rather than all
/// of it.
pub fn lexical(text: &str) -> Vec<f32> {
    let mut vector = vec![0f32; LEXICAL_DIMENSIONS];
    let normalised: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();

    for word in normalised.split_whitespace() {
        // Whole words carry more weight than their pieces.
        add(&mut vector, word, 1.5);
        let characters: Vec<char> = word.chars().collect();
        if characters.len() >= 3 {
            for window in characters.windows(3) {
                let trigram: String = window.iter().collect();
                add(&mut vector, &trigram, 1.0);
            }
        }
    }

    normalise(&mut vector);
    vector
}

fn add(vector: &mut [f32], token: &str, weight: f32) {
    let slot = (hash(token) as usize) % vector.len();
    vector[slot] += weight;
}

/// FNV-1a. Small, fast, and stable across runs, which matters because a stored
/// vector must stay comparable to one computed next week.
fn hash(text: &str) -> u64 {
    let mut value: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        value ^= *byte as u64;
        value = value.wrapping_mul(0x0000_0100_0000_01b3);
    }
    value
}

fn normalise(vector: &mut [f32]) {
    let length: f32 = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if length > 0.0 {
        for value in vector.iter_mut() {
            *value /= length;
        }
    }
}

/// Cosine similarity. Both sides are normalised, so this is a dot product.
pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Vectors are stored as little-endian f32, which is portable enough for a
/// bundle to move between machines.
pub fn to_bytes(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

pub fn from_bytes(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vector_has_the_expected_shape() {
        let vector = lexical("the ssh key lives in .ssh");
        assert_eq!(vector.len(), LEXICAL_DIMENSIONS);
        let length: f32 = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((length - 1.0).abs() < 1e-5, "vectors must be normalised");
    }

    #[test]
    fn empty_text_does_not_divide_by_zero() {
        let vector = lexical("");
        assert_eq!(vector.len(), LEXICAL_DIMENSIONS);
        assert!(vector.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn the_same_text_embeds_identically_every_time() {
        assert_eq!(lexical("backup the photos"), lexical("backup the photos"));
    }

    #[test]
    fn related_wording_scores_above_unrelated_wording() {
        let stored = lexical("the nightly backup job for /home/shahr");
        let close = similarity(&stored, &lexical("nightly backup"));
        let far = similarity(&stored, &lexical("what is the capital of Peru"));
        assert!(close > far, "close {} should beat far {}", close, far);
        assert!(close > 0.2, "a direct match should score well, got {}", close);
    }

    #[test]
    fn a_shared_stem_still_matches() {
        // Trigrams are the reason this works at all.
        let stored = lexical("backups are configured");
        let score = similarity(&stored, &lexical("backup"));
        assert!(score > 0.15, "stem match scored only {}", score);
    }

    #[test]
    fn identical_text_scores_about_one() {
        let a = lexical("compile the kernel");
        assert!((similarity(&a, &a) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn a_vector_survives_the_round_trip_to_bytes() {
        let vector = lexical("round trip me");
        let restored = from_bytes(&to_bytes(&vector));
        assert_eq!(vector, restored);
    }

    #[test]
    fn mismatched_lengths_score_zero_rather_than_panicking() {
        assert_eq!(similarity(&[1.0, 0.0], &[1.0, 0.0, 0.0]), 0.0);
    }
}
