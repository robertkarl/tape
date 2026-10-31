//! pi: `~/.pi/agent/sessions/<cwd-slug>/<ts>_<id>.jsonl`
//!
//! Verified against the real tree (2026-10-31): 20 916 `message` lines, roles
//! `user` / `assistant` / `toolResult` / `bashExecution`; envelope `timestamp`
//! is ISO-8601 while `message.timestamp` is **epoch milliseconds**; content
//! blocks are `thinking` / `text` / `toolCall` (assistant) and `text` /
//! `image` (toolResult). `model_change`, `thinking_level_change`,
//! `session_info` and `compaction` are carried into index columns, not turns —
//! except `compaction`, whose summary is real conversation content.

use std::collections::HashMap;

use serde_json::Value;

use super::*;
use crate::types::Tool;

pub fn parse_pi(raw: &str) -> anyhow::Result<Session> {
    let mut s = Session {
        tool: Tool::Pi,
        ..Default::default()
    };
    let mut range = (None, None);
    // toolCallId -> (turn index, call index within that turn)
    let mut calls: HashMap<String, (usize, usize)> = HashMap::new();

    for o in lines(raw) {
        // The session span covers every line, not just the ones that become
        // turns: a trailing toolResult or bookkeeping line is still activity.
        widen(&mut range, ts_to_ms(o.get("timestamp")));
        let ty = o.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match ty {
            "session" => {
                s.id = o
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                s.cwd = o.get("cwd").and_then(|v| v.as_str()).map(String::from);
                s.model = o.get("modelId").and_then(|v| v.as_str()).map(String::from);
            }
            "model_change" => {
                s.model = o
                    .get("modelId")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .or(s.model);
            }
            "session_info" => {
                s.name = o.get("name").and_then(|v| v.as_str()).map(String::from);
            }
            "thinking_level_change" => s.unmapped_lines += 1,
            "compaction" => {
                let ts = ts_to_ms(o.get("timestamp"));
                s.turns.push(Turn {
                    role: "system".into(),
                    ts_ms: ts,
                    text: o.get("summary").and_then(|v| v.as_str()).map(String::from),
                    thinking: None,
                    tool_calls: Vec::new(),
                    is_sidechain: false,
                });
            }
            "message" => {
                let Some(m) = o.get("message") else {
                    s.unmapped_lines += 1;
                    continue;
                };
                let ts = ts_to_ms(m.get("timestamp"));
                widen(&mut range, ts);
                let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("");
                let content = m.get("content").unwrap_or(&Value::Null);

                match role {
                    "toolResult" => {
                        let id = m.get("toolCallId").and_then(|v| v.as_str()).unwrap_or("");
                        let is_err = m.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);
                        let text = content_text(content, &["text"]);
                        if let Some((ti, ci)) = calls.get(id).copied() {
                            let tc = &mut s.turns[ti].tool_calls[ci];
                            tc.result_text = text;
                            tc.is_error = is_err;
                        } else {
                            // Orphan result (a call that scrolled out of the
                            // context window): keep it visible as its own turn.
                            s.turns.push(Turn {
                                role: "toolResult".into(),
                                ts_ms: ts,
                                text: text.clone(),
                                thinking: None,
                                tool_calls: vec![ToolCall {
                                    id: Some(id.to_string()),
                                    name: m
                                        .get("toolName")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("tool")
                                        .to_string(),
                                    args_json: "{}".into(),
                                    result_text: text.clone(),
                                    is_error: is_err,
                                }],
                                is_sidechain: false,
                            });
                        }
                    }
                    "assistant" => {
                        let mut turn = Turn {
                            role: "assistant".into(),
                            ts_ms: ts,
                            thinking: thinking_text(content, &["thinking"]),
                            text: content_text(content, &["text"]),
                            ..Default::default()
                        };
                        if let Some(err) = m.get("errorMessage").and_then(|v| v.as_str()) {
                            turn.text = match turn.text.take() {
                                Some(t) => Some(format!("{t}\n[error] {err}")),
                                None => Some(format!("[error] {err}")),
                            };
                        }
                        if let Some(model) = m.get("model").and_then(|v| v.as_str()) {
                            s.model = Some(model.to_string());
                        }
                        if let Some(u) = m.get("usage") {
                            s.in_tokens += pi_input(u);
                            s.out_tokens += u.get("output").and_then(|v| v.as_i64()).unwrap_or(0);
                            s.cost += u
                                .get("cost")
                                .and_then(|c| c.get("total"))
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0);
                        }
                        if let Value::Array(items) = content {
                            for b in items {
                                if b.get("type").and_then(|t| t.as_str()) != Some("toolCall") {
                                    continue;
                                }
                                turn.tool_calls.push(ToolCall {
                                    id: b.get("id").and_then(|v| v.as_str()).map(String::from),
                                    name: b
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("tool")
                                        .to_string(),
                                    args_json: raw_json(b.get("arguments")),
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
                    }
                    other => {
                        // `user`, `bashExecution`, and anything future: keep
                        // the role as the tool recorded it.
                        let text = content_text(content, &["text", "output"]);
                        s.turns.push(Turn {
                            role: other.to_string(),
                            ts_ms: ts,
                            text,
                            thinking: None,
                            tool_calls: Vec::new(),
                            is_sidechain: false,
                        });
                    }
                }
            }
            _ => s.unmapped_lines += 1,
        }
    }

    s.started_ms = range.0;
    s.ended_ms = range.1;
    Ok(s)
}

/// pi bills cache reads/writes separately; the index wants one comparable
/// "input" number, so fold them in (same convention as the Claude parser).
fn pi_input(u: &Value) -> i64 {
    ["input", "cacheRead", "cacheWrite"]
        .iter()
        .map(|k| u.get(*k).and_then(|v| v.as_i64()).unwrap_or(0))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PI: &str = r#"{"type":"session","version":3,"id":"019e3dc4-1","timestamp":"2026-04-22T01:23:47.000Z","cwd":"/Users/robertkarl/Code/x","modelId":"claude-opus-4-5","provider":"anthropic"}
{"type":"model_change","id":"m1","parentId":"s","timestamp":"2026-04-22T01:24:00.000Z","modelId":"claude-sonnet-4-5","provider":"anthropic"}
{"type":"session_info","id":"n1","parentId":"m1","timestamp":"2026-04-22T01:24:01.000Z","name":"fix the parser"}
{"type":"message","id":"a1","parentId":"n1","timestamp":"2026-04-22T01:24:02.000Z","message":{"role":"user","content":[{"type":"text","text":"hello"}],"timestamp":1777000000000}}
{"type":"message","id":"a2","parentId":"a1","timestamp":"2026-04-22T01:24:03.000Z","message":{"role":"assistant","model":"claude-sonnet-4-5","stopReason":"toolUse","usage":{"input":100,"output":7,"cacheRead":50,"cacheWrite":0,"totalTokens":157,"cost":{"total":0.12}},"timestamp":1777000001000,"content":[{"type":"thinking","thinking":"let me look"},{"type":"text","text":"looking"},{"type":"toolCall","id":"call_1","name":"bash","arguments":{"command":"git status | head"}}]}}
{"type":"message","id":"a3","parentId":"a2","timestamp":"2026-04-22T01:24:04.000Z","message":{"role":"toolResult","toolCallId":"call_1","toolName":"bash","isError":false,"timestamp":1777000002000,"content":[{"type":"text","text":"On branch main"}]}}
{"type":"message","id":"a4","parentId":"a3","timestamp":"2026-04-22T01:24:05.000Z","message":{"role":"assistant","stopReason":"stop","errorMessage":"stream disconnected","timestamp":1777000003000,"content":[]}}
{"type":"message","id":"a5","parentId":"a4","timestamp":"2026-04-22T01:24:06.000Z","message":{"role":"toolResult","toolCallId":"call_gone","toolName":"bash","isError":true,"timestamp":1777000004000,"content":[{"type":"text","text":"orphan"}]}}
"#;

    #[test]
    fn pi_header_and_meta() {
        let s = parse_pi(PI).unwrap();
        assert_eq!(s.id, "019e3dc4-1");
        assert_eq!(s.cwd.as_deref(), Some("/Users/robertkarl/Code/x"));
        assert_eq!(s.name.as_deref(), Some("fix the parser"));
        // last model_change / assistant model wins
        assert_eq!(s.model.as_deref(), Some("claude-sonnet-4-5"));
        assert_eq!(s.turns.iter().filter(|t| t.role == "user").count(), 1);
    }

    #[test]
    fn pi_timestamps_handle_both_formats() {
        let s = parse_pi(PI).unwrap();
        // envelope ISO-8601 and message epoch-ms both land in the same range
        let started = s.started_ms.unwrap();
        let ended = s.ended_ms.unwrap();
        assert!(ended > started, "{started} .. {ended}");
        // 1.777e12 ms is 2026 — proves the ms heuristic, not a seconds mixup
        assert!(started > 1_700_000_000_000 && started < 4_000_000_000_000);
        let first_user = s.turns.iter().find(|t| t.role == "user").unwrap();
        assert_eq!(first_user.ts_ms, Some(1_777_000_000_000));
    }

    #[test]
    fn pi_tool_results_attach_to_calls() {
        let s = parse_pi(PI).unwrap();
        let asst = s.turns.iter().find(|t| !t.tool_calls.is_empty()).unwrap();
        assert_eq!(asst.thinking.as_deref(), Some("let me look"));
        assert_eq!(asst.text.as_deref(), Some("looking"));
        let tc = &asst.tool_calls[0];
        assert_eq!(tc.name, "bash");
        assert_eq!(tc.args_json, r#"{"command":"git status | head"}"#);
        assert_eq!(tc.result_text.as_deref(), Some("On branch main"));
        assert!(!tc.is_error);
    }

    #[test]
    fn pi_orphan_result_and_error_message_survive() {
        let s = parse_pi(PI).unwrap();
        let orphan = s.turns.iter().find(|t| t.role == "toolResult").unwrap();
        assert!(orphan.tool_calls[0].is_error);
        let err = s.turns.iter().find(|t| {
            t.text.as_deref().map(|x| x.contains("stream disconnected")).unwrap_or(false)
        });
        assert!(err.is_some());
    }

    #[test]
    fn pi_usage_folds_cache_into_input() {
        let s = parse_pi(PI).unwrap();
        assert_eq!(s.in_tokens, 150); // 100 + 50 cacheRead
        assert_eq!(s.out_tokens, 7);
        assert!((s.cost - 0.12).abs() < 1e-9);
    }
}

#[cfg(test)]
mod span_tests {
    use super::*;

    #[test]
    fn session_span_covers_lines_that_never_become_turns() {
        // 1776819700000 == 2026-04-22T01:01:40Z, i.e. between the two ISO lines.
        let raw = r#"{"type":"session","id":"x","timestamp":"2026-04-22T01:00:00.000Z","cwd":"/tmp"}
{"type":"message","message":{"role":"user","content":[{"type":"text","text":"hi"}],"timestamp":1776819700000}}
{"type":"thinking_level_change","timestamp":"2026-04-22T02:00:00.000Z","thinkingLevel":"high"}
"#;
        let s = parse_pi(raw).unwrap();
        // The earliest stamp is the header (ISO); the latest belongs to a line
        // that produces no turn at all. Both mix formats, on purpose.
        assert_eq!(s.started_ms, Some(1_776_819_600_000));
        assert_eq!(
            s.ended_ms,
            ts_to_ms(Some(&serde_json::json!("2026-04-22T02:00:00.000Z")))
        );
        assert!(s.ended_ms.unwrap() > s.started_ms.unwrap());
    }
}
