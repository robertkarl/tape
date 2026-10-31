//! Shared types across the whole pipeline.
//!
//! THIS FILE IS THE CONTRACT between work units. T1 (scan/manifest) produces
//! these; T2 (store/crypto) consumes them; T3 (parsers) and T4 (transport/
//! ingest) build on them. If you need to change a shape here, say so in your
//! report — do not silently fork a second type.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Which agent tool produced a transcript.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    #[default]
    Pi,
    Claude,
    Codex,
}

impl Tool {
    pub fn as_str(&self) -> &'static str {
        match self {
            Tool::Pi => "pi",
            Tool::Claude => "claude",
            Tool::Codex => "codex",
        }
    }
}

/// One transcript file found on disk.
///
/// `group` is the archive unit. For Claude it is the session id, because a
/// session's `subagents/agent-*.jsonl` files must travel with the main file.
/// For pi and Codex a group is normally exactly one file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileEntry {
    pub tool: Tool,
    /// Session id as the producing tool reports it.
    pub session_id: String,
    /// Archive-unit key: everything with the same group is stored together.
    pub group: String,
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ms: i64,
    /// Hex blake3 of the file's exact bytes.
    pub blake3: String,
    /// False while the file is inside the quiescence window (still being
    /// written) — such files are reported but never archived.
    pub quiescent: bool,
}

/// The output of `tape scan`: what the tools have on disk right now.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub host: String,
    pub generated_at: DateTime<Utc>,
    pub entries: Vec<FileEntry>,
}

/// A stored blob: content-addressed, immutable, age-encrypted.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlobRef {
    /// Hex blake3 of the ORIGINAL plaintext bytes (the blob's identity).
    pub hash: String,
    pub path: PathBuf,
    pub ciphertext_bytes: u64,
    pub plaintext_bytes: u64,
    pub stored_at: DateTime<Utc>,
}
