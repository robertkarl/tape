//! Filesystem layout for tape's own state. Shared so T1 and T2 cannot invent
//! two different homes for the same data.
//!
//! Everything tape writes lives under one root, and nothing outside it may
//! ever be written — least of all deleted.

use std::path::PathBuf;

use crate::types::Tool;

/// Root for all tape-owned state. Override with `TAPE_HOME` for tests.
pub fn tape_home() -> PathBuf {
    if let Ok(h) = std::env::var("TAPE_HOME") {
        return PathBuf::from(h);
    }
    PathBuf::from(std::env::var("HOME").expect("HOME unset"))
        .join(".local")
        .join("share")
        .join("tapes")
}

/// Host-side record of what has already been hashed/stored (SQLite).
pub fn state_db() -> PathBuf {
    tape_home().join("state.db")
}

/// Where `tape scan` writes its manifest.
pub fn manifest_path() -> PathBuf {
    tape_home().join("manifest.json")
}

/// Content-addressed blob store: `<store>/blobs/<blake3>.tape`.
pub fn store_dir() -> PathBuf {
    tape_home().join("store")
}

pub fn blobs_dir() -> PathBuf {
    store_dir().join("blobs")
}

/// age identity (private) and the archive host's public key.
pub fn identity_path() -> PathBuf {
    tape_home().join("identity.age")
}

pub fn pubkey_path() -> PathBuf {
    tape_home().join("pubkey.txt")
}

/// Per-tool source root label, used in logs and the index.
pub fn tool_label(tool: Tool) -> &'static str {
    tool.as_str()
}
