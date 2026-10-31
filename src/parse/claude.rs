//! Claude Code: `~/.claude/projects/<cwd-slug>/<uuid>.jsonl`, plus
//! `<uuid>/subagents/agent-*.jsonl` which `scan` already groups under the
//! parent session — so this parser sees one file at a time and never has to
//! know about the directory.
//!
//! Verified against the real tree (2026-10-31): 14 346 `user` + 24 470
//! `assistant` lines carry turns; `api-request-blob` (10 325 lines) *duplicates*
//! the request payload and must never become a turn, and the file's first lines
//! are bookkeeping (`mode`, `permission-mode`, `file-history-snapshot`,
//! `queue-operation`, `ai-title`, …) rather than messages.

use std::collections::HashMap;

use serde_json::Value;

use super::*;
use crate::types::Tool;

pub fn parse_claude(raw: &str) -> anyhow::Result<Session> {
    let mut s = Session {
        tool: Tool::Claude,
        ..Default::default()
    };
    let mut range = (None, None);
    let mut calls: HashMap<String, (usize, usize)> = HashMap::new();

    for o in lines(raw) {
        // Span the whole file, including the bookkeeping lines that never
        // become turns.
        widen(&mut range, ts_to_ms(o.get("timestamp")));
        // Metadata is scattered across line types; take the first of each.
        if s.cwd.is_none() {
            s.cwd = o.get("cwd").and_then(|v| v.as_str()).map(String::from);
        }
        if s.git_branch.is_none() {
            s.git_branch = o.get("gitBranch").and_then(|v| v.as_str()).map(String::from);
        }
        if let Some(c) = o.get("totalCostUSD").and_then(|v| v.as_f64()) {
            s.cost = s.cost.max(c);
        }

        let ty = o.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if ty != "user" && ty != "assistant" {
            s.unmapped_lines += 1;
            continue;
        }
        let Some(m) = o.get("message") else {
            s.unmapped_lines += 1;
            continue;
        };
        let ts = ts_to_ms(o.get("timestamp"));
        let sidechain = o.get("isSidechain").and_then(|v| v.as_bool()).unwrap_or(false);
        let content = m.get("content").unwrap_or(&Value::Null);

        if ty == "assistant" {
            if s.model.is_none() {
                s.model = m.get("model").and_then(|v| v.as_str()).map(String::from);
            }
            if let Some(u) = m.get("usage") {
                s.in_tokens += claude_input(u);
                s.out_tokens += u
                    .get("output_tokens")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
            }
            let mut turn = Turn {
                role: "assistant".into(),
                ts_ms: ts,
                thinking: thinking_text(content, &["thinking", "redacted_thinking"]),
                text: content_text(content, &["text"]),
                is_sidechain: sidechain,
                ..Default::default()
            };
            if let Value::Array(items) = content {
                for b in items {
                    if b.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                        continue;
                    }
                    turn.tool_calls.push(ToolCall {
                        id: b.get("id").and_then(|v| v.as_str()).map(String::from),
                        name: b
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("tool")
                            .to_string(),
                        args_json: raw_json(b.get("input")),
                        result_text: None,
                        is_error: false,
                    });
                }
            }
            let ti = s.turns.len();
            for (ci, tc) in turn.tool_calls.iter().enumerate() {
                if let Some(id) = &tc.id {
                    calls.insert(id.clone(), (ti, ci));
                }
            }
            s.turns.push(turn);
            continue;
        }

        // user line: either real prompt text, or a tool_result carrier, or both.
        let text = content_text(content, &["text"]);
        let mut results = 0usize;
        if let Value::Array(items) = content {
            for b in items {
                if b.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                    continue;
                }
                results += 1;
                let id = b.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("");
                let is_err = b.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                // tool_result content is a string or a list of text/image blocks
                let out = match b.get("content") {
                    Some(Value::String(s2)) => Some(s2.clone()),
                    Some(v) => content_text(v, &["text"]),
                    None => None,
                };
                if let Some((ti, ci)) = calls.get(id).copied() {
                    let tc = &mut s.turns[ti].tool_calls[ci];
                    tc.result_text = out;
                    tc.is_error = is_err;
                } else {
                    s.turns.push(Turn {
                        role: "toolResult".into(),
                        ts_ms: ts,
                        text: out.clone(),
                        is_sidechain: sidechain,
                        tool_calls: vec![ToolCall {
                            id: Some(id.to_string()),
                            name: "tool".into(),
                            args_json: "{}".into(),
                            result_text: out,
                            is_error: is_err,
                        }],
                        ..Default::default()
                    });
                }
            }
        }
        // A tool_result-only line is plumbing, not a turn; text lines are turns.
        if text.is_some() || results == 0 {
            s.turns.push(Turn {
                role: "user".into(),
                ts_ms: ts,
                text,
                thinking: None,
                tool_calls: Vec::new(),
                is_sidechain: sidechain,
            });
        }
    }

    s.started_ms = range.0;
    s.ended_ms = range.1;
    Ok(s)
}

/// Claude bills cache reads and cache writes separately; fold them into one
/// comparable input number (same convention as the pi parser).
fn claude_input(u: &Value) -> i64 {
    [
        "input_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
    ]
    .iter()
    .map(|k| u.get(*k).and_then(|v| v.as_i64()).unwrap_or(0))
    .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: &str = r#"{"mode":"normal","sessionId":"aaaa1111","type":"mode"}
{"type":"permission-mode","permissionMode":"default","sessionId":"aaaa1111"}
{"parentUuid":null,"isSidechain":false,"type":"user","cwd":"/Users/robertkarl/Code/x","gitBranch":"main","sessionId":"aaaa1111","version":"2.1","timestamp":"2026-07-18T10:00:00.000Z","uuid":"u1","message":{"role":"user","content":"open the file"}}
{"parentUuid":"u1","isSidechain":false,"type":"assistant","cwd":"/Users/robertkarl/Code/x","gitBranch":"main","sessionId":"aaaa1111","timestamp":"2026-07-18T10:00:05.000Z","uuid":"a1","message":{"role":"assistant","model":"claude-fable-5-1","usage":{"input_tokens":2,"cache_read_input_tokens":100,"cache_creation_input_tokens":50,"output_tokens":9},"content":[{"type":"thinking","thinking":"read it first"},{"type":"text","text":"reading"},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"cat src/main.rs","description":"show file"}}]}}
{"parentUuid":"a1","isSidechain":false,"type":"user","sessionId":"aaaa1111","timestamp":"2026-07-18T10:00:06.000Z","uuid":"u2","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"fn main() {}","is_error":false}]}}
{"parentUuid":"a1","isSidechain":true,"type":"assistant","sessionId":"aaaa1111","timestamp":"2026-07-18T10:00:07.000Z","uuid":"s1","message":{"role":"assistant","model":"claude-fable-5-1","content":[{"type":"text","text":"sidechain says hi"}]}}
{"type":"api-request-blob","sessionId":"aaaa1111","message":{"role":"assistant","content":[{"type":"text","text":"DUPLICATE MUST NOT APPEAR"}]}}
{"type":"file-history-snapshot","sessionId":"aaaa1111","snapshot":{}}
"#;

    #[test]
    fn claude_skips_bookkeeping_and_api_blobs() {
        let s = parse_claude(CLAUDE).unwrap();
        assert!(s
            .turns
            .iter()
            .all(|t| t.text.as_deref() != Some("DUPLICATE MUST NOT APPEAR")));
        assert!(s.unmapped_lines >= 3, "bookkeeping lines are counted");
        assert_eq!(s.cwd.as_deref(), Some("/Users/robertkarl/Code/x"));
        assert_eq!(s.git_branch.as_deref(), Some("main"));
        assert_eq!(s.model.as_deref(), Some("claude-fable-5-1"));
    }

    #[test]
    fn claude_tool_results_attach_to_tool_use() {
        let s = parse_claude(CLAUDE).unwrap();
        let asst = s.turns.iter().find(|t| !t.tool_calls.is_empty()).unwrap();
        assert_eq!(asst.thinking.as_deref(), Some("read it first"));
        let tc = &asst.tool_calls[0];
        assert_eq!(tc.name, "Bash");
        assert_eq!(tc.args_json, r#"{"command":"cat src/main.rs","description":"show file"}"#);
        assert_eq!(tc.result_text.as_deref(), Some("fn main() {}"));
        // the tool_result line did not become a user turn
        assert_eq!(
            s.turns
                .iter()
                .filter(|t| t.role == "user" && t.text.is_some())
                .count(),
            1
        );
    }

    #[test]
    fn claude_sidechain_is_recorded() {
        let s = parse_claude(CLAUDE).unwrap();
        let side = s.turns.iter().filter(|t| t.is_sidechain).count();
        assert_eq!(side, 1);
    }

    #[test]
    fn claude_usage_folds_cache() {
        let s = parse_claude(CLAUDE).unwrap();
        assert_eq!(s.in_tokens, 152);
        assert_eq!(s.out_tokens, 9);
    }
}
