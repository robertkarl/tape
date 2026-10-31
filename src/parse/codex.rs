//! Codex: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` and
//! `~/.codex/archived_sessions/`.
//!
//! Verified against the real tree (2026-10-31): every line is
//! `{timestamp, type, payload}`. Only `response_item` feeds turns — `event_msg`
//! (86 766 lines) re-reports the same content as `agent_message` /
//! `user_message` / `exec_command_end`, and `token_usage_record`,
//! `turn_context`, `session_meta`, `world_state` and `compacted` are metadata.
//! Turning `event_msg` into turns would double every session, so it is counted
//! as unmapped and left in the blob.

use std::collections::HashMap;

use serde_json::Value;

use super::*;
use crate::types::Tool;

pub fn parse_codex(raw: &str) -> anyhow::Result<Session> {
    let mut s = Session {
        tool: Tool::Codex,
        ..Default::default()
    };
    let mut range = (None, None);
    let mut calls: HashMap<String, (usize, usize)> = HashMap::new();

    for o in lines(raw) {
        let ts = ts_to_ms(o.get("timestamp"));
        widen(&mut range, ts);
        let ty = o.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let p = o.get("payload").cloned().unwrap_or(Value::Null);

        match ty {
            "session_meta" => {
                s.id = p
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                s.cwd = p.get("cwd").and_then(|v| v.as_str()).map(String::from);
            }
            "turn_context" => {
                if s.model.is_none() {
                    s.model = p.get("model").and_then(|v| v.as_str()).map(String::from);
                }
                if s.cwd.is_none() {
                    s.cwd = p.get("cwd").and_then(|v| v.as_str()).map(String::from);
                }
                s.unmapped_lines += 1;
            }
            "token_usage_record" => {
                if let Some(u) = p.get("usage") {
                    s.in_tokens += u
                        .get("input_tokens")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    s.out_tokens += u
                        .get("output_tokens")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                }
                s.unmapped_lines += 1;
            }
            "response_item" => {
                let pt = p.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match pt {
                    "message" => {
                        let role = p.get("role").and_then(|v| v.as_str()).unwrap_or("assistant");
                        let text = content_text(
                            p.get("content").unwrap_or(&Value::Null),
                            &["input_text", "output_text", "text"],
                        );
                        s.turns.push(Turn {
                            role: role.to_string(),
                            ts_ms: ts,
                            text,
                            ..Default::default()
                        });
                    }
                    "reasoning" => {
                        // Older builds put plain text here; newer ones ship
                        // only `encrypted_content`, which we cannot model.
                        let thinking = p
                            .get("content")
                            .and_then(|c| content_text(c, &["summary_text", "text"]))
                            .or_else(|| {
                                p.get("summary").and_then(|c| content_text(c, &["summary_text"]))
                            });
                        if thinking.is_none() {
                            s.unmapped_lines += 1;
                            continue;
                        }
                        match s.turns.last_mut() {
                            Some(t) if t.role == "assistant" && t.thinking.is_none() => {
                                t.thinking = thinking
                            }
                            _ => s.turns.push(Turn {
                                role: "assistant".into(),
                                ts_ms: ts,
                                thinking,
                                ..Default::default()
                            }),
                        }
                    }
                    "function_call" | "custom_tool_call" | "web_search_call" | "local_shell_call"
                    | "tool_search_call" => {
                        let name = p
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or(pt)
                            .to_string();
                        let args = if pt == "custom_tool_call" {
                            p.get("input")
                                .map(|v| match v {
                                    Value::String(s2) => s2.clone(),
                                    v => v.to_string(),
                                })
                                .unwrap_or_else(|| "{}".into())
                        } else if pt == "web_search_call" {
                            p.get("action")
                                .map(|v| v.to_string())
                                .unwrap_or_else(|| "{}".into())
                        } else {
                            raw_json(p.get("arguments"))
                        };
                        let call = ToolCall {
                            id: p.get("call_id").and_then(|v| v.as_str()).map(String::from),
                            name,
                            args_json: args,
                            result_text: None,
                            is_error: false,
                        };
                        let ti = s.turns.len();
                        s.turns.push(Turn {
                            role: "assistant".into(),
                            ts_ms: ts,
                            tool_calls: vec![call],
                            ..Default::default()
                        });
                        if let Some(id) = s.turns[ti].tool_calls[0].id.clone() {
                            calls.insert(id, (ti, 0));
                        }
                    }
                    "function_call_output" | "custom_tool_call_output" | "web_search_end"
                    | "exec_command_end" => {
                        let out = match p.get("output") {
                            Some(Value::String(s2)) => Some(s2.clone()),
                            Some(v) => content_text(v, &["output_text", "text"]),
                            None => None,
                        };
                        let is_err = p
                            .get("is_error")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let id = p.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                        if let Some((ti, ci)) = calls.get(id).copied() {
                            let tc = &mut s.turns[ti].tool_calls[ci];
                            tc.result_text = out;
                            tc.is_error = is_err;
                        } else {
                            s.turns.push(Turn {
                                role: "toolResult".into(),
                                ts_ms: ts,
                                text: out.clone(),
                                tool_calls: vec![ToolCall {
                                    id: Some(id.to_string()),
                                    name: pt.to_string(),
                                    args_json: "{}".into(),
                                    result_text: out,
                                    is_error: is_err,
                                }],
                                ..Default::default()
                            });
                        }
                    }
                    _ => s.unmapped_lines += 1,
                }
            }
            // event_msg duplicates the same content; never a turn.
            _ => s.unmapped_lines += 1,
        }
    }

    s.started_ms = range.0;
    s.ended_ms = range.1;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODEX: &str = r#"{"timestamp":"2026-03-07T22:40:34.965Z","type":"session_meta","payload":{"id":"019cca75-aa94","timestamp":"2026-03-07T22:40:34.965Z","cwd":"/Users/robertkarl/Code/x","originator":"Codex Desktop","cli_version":"0.108.0","base_instructions":{"text":"You are Codex"}}}
{"timestamp":"2026-03-07T22:40:35.000Z","type":"turn_context","payload":{"turn_id":"t1","cwd":"/Users/robertkarl/Code/x","model":"gpt-5.4","approval_policy":"on-request"}}
{"timestamp":"2026-03-07T22:40:36.000Z","type":"event_msg","payload":{"type":"user_message","message":"DUPLICATE MUST NOT APPEAR"}}
{"timestamp":"2026-03-07T22:40:37.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"list the readmes"}]}}
{"timestamp":"2026-03-07T22:40:38.000Z","type":"response_item","payload":{"type":"reasoning","summary":[],"encrypted_content":"gAAAA..."}}
{"timestamp":"2026-03-07T22:40:39.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"rg --files -g 'README*'\"}","call_id":"call_1"}}
{"timestamp":"2026-03-07T22:40:40.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call_1","output":"README.md\n"}}
{"timestamp":"2026-03-07T22:40:41.000Z","type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","input":"*** Begin Patch\n*** End Patch\n","call_id":"call_2"}}
{"timestamp":"2026-03-07T22:40:42.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"one readme"}]}}
{"timestamp":"2026-03-07T22:40:43.000Z","type":"token_usage_record","payload":{"thread_id":"019cca75-aa94","usage":{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":25}}}
{"timestamp":"2026-03-07T22:40:44.000Z","type":"event_msg","payload":{"type":"agent_message","message":"DUPLICATE MUST NOT APPEAR"}}
"#;

    #[test]
    fn codex_meta_and_model() {
        let s = parse_codex(CODEX).unwrap();
        assert_eq!(s.id, "019cca75-aa94");
        assert_eq!(s.cwd.as_deref(), Some("/Users/robertkarl/Code/x"));
        assert_eq!(s.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(s.in_tokens, 1000);
        assert_eq!(s.out_tokens, 25);
    }

    #[test]
    fn codex_event_msgs_never_become_turns() {
        let s = parse_codex(CODEX).unwrap();
        assert!(s
            .turns
            .iter()
            .all(|t| t.text.as_deref() != Some("DUPLICATE MUST NOT APPEAR")));
        assert_eq!(s.turns.iter().filter(|t| t.role == "user").count(), 1);
    }

    #[test]
    fn codex_calls_and_outputs_pair_up() {
        let s = parse_codex(CODEX).unwrap();
        let calls: Vec<&ToolCall> = s.turns.iter().flat_map(|t| &t.tool_calls).collect();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "exec_command");
        assert_eq!(calls[0].result_text.as_deref(), Some("README.md\n"));
        assert_eq!(calls[1].name, "apply_patch");
        assert!(calls[1].args_json.starts_with("*** Begin Patch"));
    }

    #[test]
    fn codex_encrypted_reasoning_is_counted_not_invented() {
        let s = parse_codex(CODEX).unwrap();
        assert!(s.turns.iter().all(|t| t.thinking.is_none()));
        assert!(s.unmapped_lines > 0);
    }
}
