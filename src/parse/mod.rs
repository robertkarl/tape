//! PARSERS — owned by T3. Raw JSONL stays verbatim in the blob; this module
//! only projects it into the canonical model below, for the index and the
//! renderer. Fidelity is Layer 1's job, never this one's.
//!
//! A parse error never destroys anything: the blob is the source of truth and
//! the index is disposable. Parsers are deliberately tolerant of unknown line
//! types — they ignore what they cannot model rather than failing the file.
//!
//! Traps already paid for (see PLAN.md §6):
//!   * pi: envelope `timestamp` is ISO-8601 but `message.timestamp` is
//!     EPOCH MILLISECONDS — use the `> 1e11 => ms` heuristic (`ts_to_ms`).
//!   * claude: `isSidechain` turns and `<session>/subagents/agent-*.jsonl`
//!     belong to the parent session; `api-request-blob` lines duplicate the
//!     request payload and must not become turns.
//!   * codex: only `response_item` feeds turns; `session_meta`, `turn_context`,
//!     `world_state`, `token_usage_record` become index columns.

pub mod claude;
pub mod codex;
pub mod pi;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub tool: crate::types::Tool,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub model: Option<String>,
    /// Human label if the tool records one (pi `session_info`).
    pub name: Option<String>,
    pub started_ms: Option<i64>,
    pub ended_ms: Option<i64>,
    pub turns: Vec<Turn>,
    pub in_tokens: i64,
    pub out_tokens: i64,
    pub cost: f64,
    /// Lines the parser did not model (kept in the blob, counted for honesty).
    pub unmapped_lines: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Turn {
    pub role: String,
    pub ts_ms: Option<i64>,
    pub thinking: Option<String>,
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    /// Claude subagent turns; surfaced so the UI can group them.
    pub is_sidechain: bool,
}

impl Default for Turn {
    fn default() -> Self {
        Turn {
            role: String::new(),
            ts_ms: None,
            thinking: None,
            text: None,
            tool_calls: Vec::new(),
            is_sidechain: false,
        }
    }
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

pub fn parse_for(tool: crate::types::Tool, raw: &str) -> anyhow::Result<Session> {
    match tool {
        crate::types::Tool::Pi => pi::parse_pi(raw),
        crate::types::Tool::Claude => claude::parse_claude(raw),
        crate::types::Tool::Codex => codex::parse_codex(raw),
    }
}

/// Every tool records timestamps differently, and pi records them two ways in
/// the same object. Numbers are epoch-ms when large, epoch-s when small;
/// strings are ISO-8601.
pub fn ts_to_ms(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                // `> 1e11` is unambiguously milliseconds for any real date.
                Some(if i > 100_000_000_000 { i } else { i * 1000 })
            } else {
                n.as_f64().map(|f| f as i64)
            }
        }
        Value::String(s) => {
            if let Ok(d) = chrono::DateTime::parse_from_rfc3339(s) {
                return Some(d.timestamp_millis());
            }
            // Some tools emit "2026-03-07" (date only) in context blocks.
            if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
                return Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis());
            }
            None
        }
        _ => None,
    }
}

/// Text out of a `content` field that may be a bare string or a list of blocks.
/// Thinking blocks store their prose under `thinking`, not `text`, so the
/// field name is a parameter, not an assumption.
pub fn blocks(content: &Value, block_types: &[&str], fields: &[&str]) -> Option<String> {
    match content {
        Value::String(s) if !s.is_empty() && fields.contains(&"text") => Some(s.clone()),
        Value::Array(items) => {
            let mut parts: Vec<String> = Vec::new();
            for b in items {
                if let Some(t) = b.get("type").and_then(|t| t.as_str()) {
                    if !block_types.contains(&t) {
                        continue;
                    }
                    for f in fields {
                        if let Some(s) = b.get(*f).and_then(|s| s.as_str()) {
                            if !s.is_empty() {
                                parts.push(s.to_string());
                                break;
                            }
                        }
                    }
                } else if let Value::String(s) = b {
                    if !s.is_empty() {
                        parts.push(s.clone());
                    }
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            }
        }
        _ => None,
    }
}

pub fn content_text(content: &Value, block_types: &[&str]) -> Option<String> {
    blocks(content, block_types, &["text"])
}

/// Prose from thinking blocks (`thinking` field, as pi and Claude both store it).
pub fn thinking_text(content: &Value, block_types: &[&str]) -> Option<String> {
    blocks(content, block_types, &["thinking", "text"])
}

/// A short human-meaningful label for a tool call, for the index and the list
/// view: pipeline verbs for shells, file basename for file tools.
pub fn verb_for(name: &str, args_json: &str) -> Option<String> {
    let args: Value = serde_json::from_str(args_json).unwrap_or(Value::Null);
    let field = |keys: &[&str]| -> Option<String> {
        keys.iter().find_map(|k| {
            args.get(*k)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
    };
    match name {
        "bash" | "shell" | "exec" | "exec_command" | "local_shell" | "Bash" => {
            let cmd = field(&["command", "cmd", "script"])?;
            first_pipeline_verb(&cmd)
        }
        "apply_patch" => Some("patch".to_string()),
        "read" | "Read" | "write" | "Write" | "edit" | "Edit" | "str_replace_editor"
        | "NotebookEdit" => {
            let p = field(&["file_path", "path", "notebook_path"])?;
            std::path::Path::new(&p)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
        }
        _ => None,
    }
}

fn first_pipeline_verb(cmd: &str) -> Option<String> {
    use std::path::Path;
    let seg = cmd
        .split(['|', '\n', ';'])
        .map(str::trim)
        .find(|s| !s.is_empty())?;
    let mut it = seg.split_whitespace();
    let first = it.next()?;
    // `git commit -m ...` is more useful as "git commit" than "git".
    if matches!(first, "git" | "cargo" | "docker" | "zfs" | "pct") {
        if let Some(sub) = it.next() {
            return Some(format!("{first} {}", sub.trim_start_matches('-')));
        }
    }
    Some(
        Path::new(first)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| first.to_string()),
    )
}

/// Track min/max timestamps without letting a missing value reset the range.
pub fn widen(range: &mut (Option<i64>, Option<i64>), ts: Option<i64>) {
    let Some(ts) = ts else { return };
    range.0 = Some(range.0.map(|m| m.min(ts)).unwrap_or(ts));
    range.1 = Some(range.1.map(|m| m.max(ts)).unwrap_or(ts));
}

/// Keep an argument envelope exactly as recorded, compactly.
pub(crate) fn raw_json(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "{}".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
    }
}

/// Iterate the parseable lines of a JSONL transcript, counting what was skipped.
pub fn lines(raw: &str) -> impl Iterator<Item = Value> + '_ {
    raw.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| {
        serde_json::from_str(l).ok()
    })
}
