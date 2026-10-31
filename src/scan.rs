//! SOURCE DISCOVERY — owned by T1.
//!
//! Walk the three tools' transcript roots, group Claude subagent files with
//! their parent session, hash, and apply the quiescence window.
//! See TASKS.md T1 for acceptance criteria.

use std::path::PathBuf;

use crate::types::Manifest;

/// Minutes a file must be untouched before we archive it. Transcript files are
/// appended to while a session runs; a partial tail is worse than a late copy.
pub const QUIESCENCE_MINUTES: u64 = 10;

/// Home-directory-rooted source roots, expanded at runtime.
pub fn source_roots() -> Vec<(crate::types::Tool, PathBuf)> {
    let home = dirs_home();
    vec![
        (
            crate::types::Tool::Claude,
            PathBuf::from(&home).join(".claude/projects"),
        ),
        (
            crate::types::Tool::Codex,
            PathBuf::from(&home).join(".codex/sessions"),
        ),
        (
            crate::types::Tool::Codex,
            PathBuf::from(&home).join(".codex/archived_sessions"),
        ),
        (
            crate::types::Tool::Pi,
            PathBuf::from(&home).join(".pi/agent/sessions"),
        ),
    ]
}

fn dirs_home() -> String {
    std::env::var("HOME").expect("HOME unset")
}

/// T1: produce the manifest. Must NOT write outside the state dir.
pub fn scan() -> anyhow::Result<Manifest> {
    anyhow::bail!("not implemented (T1): scan the source roots into a Manifest")
}
