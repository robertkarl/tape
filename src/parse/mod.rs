//! PARSERS — owned by T3. Raw JSONL stays verbatim in the blob; this module
//! only projects it into the canonical model below, for the index and the
//! renderer. Fidelity is Layer 1's job, never this one's.
//!
//! Traps already paid for (see PLAN.md §6):
//!   * pi: envelope `timestamp` is ISO-8601 but `message.timestamp` is
//!     EPOCH MILLISECONDS — use the `> 1e11 => ms` heuristic.
//!   * claude: `isSidechain` turns and `<session>/subagents/agent-*.jsonl`
//!     belong to the parent session.
//!   * codex: only `response_item` feeds turns; `session_meta`,
//!     `turn_context`, `token_usage_record` become index columns.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub tool: crate::types::Tool,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub model: Option<String>,
    pub started_ms: Option<i64>,
    pub ended_ms: Option<i64>,
    pub turns: Vec<Turn>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Turn {
    pub role: String,
    pub ts_ms: Option<i64>,
    pub thinking: Option<String>,
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: Option<String>,
    pub name: String,
    /// Raw JSON argument envelope, exactly as the tool recorded it.
    pub args_json: String,
    pub result_text: Option<String>,
    pub is_error: bool,
}

pub fn parse_pi(raw: &str) -> anyhow::Result<Session> {
    let _ = raw;
    anyhow::bail!("not implemented (T3)")
}

pub fn parse_claude(raw: &str) -> anyhow::Result<Session> {
    let _ = raw;
    anyhow::bail!("not implemented (T3)")
}

pub fn parse_codex(raw: &str) -> anyhow::Result<Session> {
    let _ = raw;
    anyhow::bail!("not implemented (T3)")
}
