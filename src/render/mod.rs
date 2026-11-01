//! RENDERER — port of `~/Code/regina/scripts/render_trace.py` (regina, 2026).
//!
//! The template, the density modes, the one-turn-at-a-time paging, the
//! resizable timeline rail, the emoji-per-tool badges, edits-as-diffs and the
//! long-output collapse are that file's design, translated rather than
//! redesigned. What is genuinely new here is what the archive has and regina's
//! runs did not: Claude subagent turns, Codex text patches, and multi-blob
//! sessions (a Claude session and its subagent files arrive as separate blobs
//! under one session id).
//!
//! Input is the canonical `parse::Session`, never a source tree. That is the
//! whole point: the archive host has blobs and an index, not the repo.

pub mod badges;
pub mod highlight;
pub mod template;

use crate::parse::Session;
use serde_json::Value;
use std::collections::BTreeMap;

pub use highlight::esc;

/// Output longer than this collapses behind a `<details>`; the summary carries
/// the line and char counts so you know what you are declining to open.
const COLLAPSE_CHARS: usize = 1400;

/// Tool arguments as the tool recorded them. Most are JSON; Codex patches are
/// raw text, and pretending otherwise would put a patch through a JSON parser
/// and print the error.
enum Args {
    Json(Value),
    Raw(String),
}

fn parse_args(raw: &str) -> Args {
    match serde_json::from_str::<Value>(raw) {
        Ok(v @ Value::Object(_)) => Args::Json(v),
        Ok(v) => Args::Json(Value::Object(
            serde_json::map::Map::from_iter([("raw".to_string(), v)]),
        )),
        Err(_) => Args::Raw(raw.to_string()),
    }
}

impl Args {
    fn get<'a>(&'a self, k: &str) -> Option<&'a Value> {
        match self {
            Args::Json(v) => v.get(k),
            Args::Raw(_) => None,
        }
    }
    fn str_field(&self, k: &str) -> Option<String> {
        self.get(k).and_then(|v| v.as_str()).map(String::from)
    }
    fn as_value(&self) -> Value {
        match self {
            Args::Json(v) => v.clone(),
            Args::Raw(s) => Value::String(s.clone()),
        }
    }
}

/// What the timeline shows for a call: the command or path, single-lined, and
/// for file tools the BASENAME only — every path in a session shares a prefix,
/// and the leading dirs push the filename out of the rail. The full text stays
/// on hover.
fn shown_key(name: &str, key: &str) -> String {
    let flat = key.split_whitespace().collect::<Vec<_>>().join(" ");
    if badges::FILE_TOOLS.contains(&name.to_lowercase().as_str()) {
        let base = flat.rsplit('/').next().unwrap_or("").to_string();
        if !base.is_empty() {
            return base;
        }
    }
    flat
}

fn page_title(s: &Session) -> String {
    let day = s
        .started_ms
        .map(|ms| {
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let project = s
        .cwd
        .as_deref()
        .and_then(|c| c.split('/').next_back())
        .filter(|c| !c.is_empty())
        .unwrap_or(s.tool.as_str());
    let model = s.model.clone().unwrap_or_else(|| "?".into());
    let mut parts = vec![format!("{model} - {project}")];
    if !day.is_empty() {
        parts.push(day);
    }
    parts.push(s.id.clone());
    parts.join(" - ")
}

/// One session page, self-contained: no external assets, no CDN.
pub fn render_session(s: &Session, host: &str) -> anyhow::Result<String> {
    let t0 = s.turns.iter().find_map(|t| t.ts_ms);
    let mut prev = t0;
    let mut turn_no = 0usize;

    let mut turns_html = String::new();
    let mut timeline_html = String::new();
    let mut reps: BTreeMap<(String, String), u64> = Default::default();
    let mut call_count = 0usize;

    for t in &s.turns {
        turn_no += 1;
        let offset = match (t.ts_ms, t0) {
            (Some(ts), Some(t0)) => Some((ts - t0) as f64 / 1000.0),
            _ => None,
        };
        let dur = match (t.ts_ms, prev) {
            (Some(ts), Some(p)) => Some((ts - p) as f64 / 1000.0),
            _ => None,
        };
        prev = t.ts_ms.or(prev);

        let mut blocks = String::new();
        let mut tool_badges: Vec<(String, String, String, String)> = Vec::new();

        if let Some(th) = &t.thinking {
            if !th.trim().is_empty() {
                blocks.push_str(&format!(
                    "<details class=\"think\"><summary>💭 thinking ({} chars)</summary>\
                     <div class=\"think-body\">{}</div></details>",
                    th.chars().count(),
                    esc(th)
                ));
            }
        }
        if let Some(tx) = &t.text {
            if !tx.trim().is_empty() {
                blocks.push_str(&format!("<div class=\"say\">{}</div>", esc(tx)));
            }
        }

        for tc in &t.tool_calls {
            call_count += 1;
            let args = parse_args(&tc.args_json);
            let key = match &args {
                Args::Json(_) => args
                    .str_field("command")
                    .or_else(|| args.str_field("cmd"))
                    .or_else(|| args.str_field("path"))
                    .or_else(|| args.str_field("file_path"))
                    .or_else(|| {
                        Some(
                            tc.args_json
                                .chars()
                                .take(80)
                                .collect::<String>(),
                        )
                    }),
                Args::Raw(s) => Some(s.chars().take(80).collect::<String>()),
            }
            .unwrap_or_default();
            *reps
                .entry((tc.name.clone(), key.chars().take(90).collect()))
                .or_insert(0) += 1;

            let (emoji, vlabel) = badges::tool_icon(&tc.name, &args.as_value());
            let shown = shown_key(&tc.name, &key);
            let full = key.split_whitespace().collect::<Vec<_>>().join(" ");
            tool_badges.push((emoji.clone(), vlabel.clone(), shown, full));

            let out = tc.result_text.clone().unwrap_or_default();
            let out_cls = if tc.is_error { "out err" } else { "out" };
            let out_pre = if out.trim().is_empty() {
                "<pre class=\"code out empty\">(no output)</pre>".to_string()
            } else {
                format!(
                    "<pre class=\"code {out_cls}\">{}</pre>",
                    highlight::highlight(&out, "output")
                )
            };
            let out_block = if out.chars().count() > COLLAPSE_CHARS {
                format!(
                    "<details class=\"outwrap\"><summary>output · {} lines · {} chars</summary>{}</details>",
                    out.split('\n').count(),
                    out.chars().count(),
                    out_pre
                )
            } else {
                format!("<div class=\"outwrap\">{out_pre}</div>")
            };

            blocks.push_str(&format!(
                "<div class=\"tool\"><div class=\"tool-hd\"><span class=\"temoji\">{emoji}</span>\
                 <span class=\"tname\">{}</span>\
                 <span class=\"tverb\">{}</span>{}</div>\
                 <div class=\"io\"><div class=\"in\">{}</div><div class=\"io-out\">{out_block}</div></div></div>",
                esc(&tc.name),
                if vlabel == tc.name.to_lowercase() { String::new() } else { esc(&vlabel) },
                if tc.is_error { " ⚠️" } else { "" },
                render_tool_input(&tc.name, &args),
            ));
        }

        // Timeline badge: emoji + the ACTUAL command/path, so the rail is
        // readable on its own and a bash/bash/bash run still varies by icon.
        let title: String;
        let mut badge_html: String;
        if !tool_badges.is_empty() {
            let (em, _lbl, shown, full) = &tool_badges[0];
            title = if full.is_empty() { _lbl.clone() } else { full.clone() };
            badge_html = format!(
                "<span class=\"tl-em\">{em}</span><span class=\"tl-cmd\">{}</span>",
                if shown.is_empty() { esc(_lbl) } else { esc(shown) }
            );
            if tool_badges.len() > 1 {
                badge_html.push_str(&format!(
                    "<span class=\"tl-more\">+{}</span>",
                    tool_badges.len() - 1
                ));
            }
        } else if t.role == "assistant" {
            title = "assistant message".into();
            badge_html = "<span class=\"tl-em\">💬</span><span class=\"tl-cmd\">say</span>".into();
        } else {
            title = t.role.clone();
            badge_html = format!("<span class=\"tl-cmd\">{}</span>", esc(&t.role));
        }

        let off_s = offset.map(|o| format!("{o:.1}s")).unwrap_or_else(|| "—".into());
        let dur_s = dur.map(|d| format!("{d:.1}s")).unwrap_or_default();
        let rolecls = if t.role == "assistant" { "asst" } else { "user" };
        let side = if t.is_sidechain { " side" } else { "" };

        timeline_html.push_str(&format!(
            "<a class=\"tl-row {rolecls}{side}\" href=\"#t{turn_no}\" title=\"{}\">\
             <span class=\"tl-n\">{turn_no}</span><span class=\"tl-off\">{off_s}</span>\
             <span class=\"tl-badge\">{badge_html}</span><span class=\"tl-dur\">{dur_s}</span></a>",
            esc(&title)
        ));

        // The skim line is the SAME badge the rail shows, so Skim mode is a CSS
        // switch rather than a second rendering pass.
        turns_html.push_str(&format!(
            "<section class=\"turn {rolecls}{side}\" id=\"t{turn_no}\" data-turn=\"{turn_no}\">\
             <div class=\"turn-hd\"><span class=\"tn\">#{turn_no}</span>\
             <span class=\"troll\">{}</span>{}<span class=\"toff\">{off_s}</span>\
             <span class=\"tdur\">{}</span></div>\
             <div class=\"skim\">{badge_html}</div>\
             <div class=\"turn-body\">{blocks}</div></section>",
            esc(&t.role),
            if t.is_sidechain {
                "<span class=\"tside\">subagent</span>"
            } else {
                ""
            },
            if dur_s.is_empty() {
                String::new()
            } else {
                format!("Δ {dur_s}")
            }
        ));
    }

    // Repeats: the model doing the same dumb thing N times.
    let mut rep_rows: Vec<(String, String, u64)> = reps
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(k, n)| (k.0, k.1, n))
        .collect();
    rep_rows.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    let rows: String = rep_rows
        .into_iter()
        .take(15)
        .map(|(name, cmd, n)| {
            format!(
                "<tr><td class=\"rn\">{n}×</td><td class=\"rt\">{}</td><td class=\"rc\"><code>{}</code></td></tr>",
                esc(&name),
                esc(&cmd)
            )
        })
        .collect();
    let rep_html = if rows.is_empty() {
        "<p class=\"none\">no repeated tool inputs</p>".to_string()
    } else {
        format!("<table class=\"reps\">{rows}</table>")
    };

    let wall = match (s.started_ms, s.ended_ms) {
        (Some(a), Some(b)) if b >= a => format!("{}s", (b - a) / 1000),
        _ => "—".into(),
    };

    let mut extra = String::new();
    if let Some(b) = &s.git_branch {
        extra.push_str(&format!("<span>branch <b>{}</b></span>", esc(b)));
    }
    if s.in_tokens > 0 || s.out_tokens > 0 {
        extra.push_str(&format!(
            "<span>tokens <b>{} in / {} out</b></span>",
            s.in_tokens, s.out_tokens
        ));
    }
    if s.cost > 0.0 {
        extra.push_str(&format!("<span>cost <b>${:.2}</b></span>", s.cost));
    }
    if s.turns.iter().any(|t| t.is_sidechain) {
        extra.push_str("<span>subagent turns <b>yes</b></span>");
    }

    let body = if s.turns.is_empty() {
        // Real: Claude writes bookkeeping-only files, and Codex sessions start
        // before anything is said. An empty page must say why, not just be blank.
        "<p class=\"none\">this session has no turns the parser could model</p>".to_string()
    } else {
        turns_html
    };

    Ok(template::fill(
        template::PAGE,
        &[
            ("TITLE", &esc(&page_title(s))),
            ("MODEL", &esc(s.model.as_deref().unwrap_or("(model not recorded)"))),
            ("CWD", &esc(s.cwd.as_deref().unwrap_or("—"))),
            ("TOOL", &esc(s.tool.as_str())),
            ("HOST", &esc(host)),
            ("EXTRA", &extra),
            ("SCORE", &format!("{call_count} tool calls")),
            ("STCLS", "st-ok"),
            ("STATUS", "archived"),
            ("WALL", &wall),
            ("TURNS", &s.turns.len().to_string()),
            ("TIMELINE", &timeline_html),
            ("REPS", &rep_html),
            ("BODY", &body),
        ],
    ))
}

/// The `<pre>` for a tool call's input, highlighted by best-guess language.
///
/// JSON is technically faithful but cognitively hostile for the most important
/// operation in a coding trace, so edits become old/new diffs. The original
/// knew pi's `edits` array; the archive also has Claude's `old_string`/
/// `new_string` and Codex's text patches, which get the same treatment.
fn render_tool_input(name: &str, args: &Args) -> String {
    let n = name.to_lowercase();
    let raw_text = match args {
        Args::Raw(s) => Some(s.clone()),
        Args::Json(_) => None,
    };

    if matches!(n.as_str(), "bash" | "shell" | "sh" | "local_shell_call") {
        let body = badges::command_of(&args.as_value()).to_string();
        return format!(
            "<pre class=\"code lang-bash\">{}</pre>",
            highlight::highlight(&body, "bash")
        );
    }

    // pi: `edits: [{oldText,newText},…]`
    if matches!(n.as_str(), "edit" | "edit_file" | "str_replace") {
        if let Some(edits) = args.get("edits").and_then(|v| v.as_array()) {
            return edit_diff(name, args, edits);
        }
        // Claude: one old_string/new_string pair per call.
        let old = args.str_field("old_string").or_else(|| args.str_field("oldText"));
        let new = args.str_field("new_string").or_else(|| args.str_field("newText"));
        if old.is_some() || new.is_some() {
            let edits = vec![Value::Object(serde_json::map::Map::from_iter([
                ("oldText".to_string(), Value::String(old.unwrap_or_default())),
                ("newText".to_string(), Value::String(new.unwrap_or_default())),
            ]))];
            return edit_diff(name, args, &edits);
        }
    }

    if let Some(body) = args.str_field("content") {
        let lang = badges::guess_lang(name, &body);
        return pre_for(&body, lang);
    }

    // A read: the interesting input is the path, not four JSON fields.
    let keys: Vec<String> = match args {
        Args::Json(Value::Object(m)) => m.keys().cloned().collect(),
        _ => Vec::new(),
    };
    if !keys.is_empty()
        && keys
            .iter()
            .all(|k| matches!(k.as_str(), "path" | "offset" | "limit" | "file_path"))
    {
        let body = args
            .str_field("path")
            .or_else(|| args.str_field("file_path"))
            .unwrap_or_default();
        return pre_for(&body, "text");
    }

    if let Some(body) = raw_text {
        return pre_for(&body, badges::guess_lang(name, &body));
    }

    let body = serde_json::to_string_pretty(&args.as_value()).unwrap_or_else(|_| tc_args_inline(args));
    pre_for(&body, "text")
}

fn tc_args_inline(args: &Args) -> String {
    match args {
        Args::Raw(s) => s.clone(),
        Args::Json(v) => v.to_string(),
    }
}

fn pre_for(body: &str, lang: &str) -> String {
    let inner = if lang == "diff" && highlight::looks_like_patch(body) {
        highlight::patch(body)
    } else {
        highlight::highlight(body, lang)
    };
    format!("<pre class=\"code lang-{lang}\">{inner}</pre>")
}

fn edit_diff(name: &str, args: &Args, edits: &[Value]) -> String {
    let path = args
        .str_field("path")
        .or_else(|| args.str_field("file_path"))
        .unwrap_or_else(|| "(path not supplied)".into());
    let mut changes = String::new();
    for (i, edit) in edits.iter().enumerate() {
        let old = edit
            .get("oldText")
            .or_else(|| edit.get("old_text"))
            .or_else(|| edit.get("old"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let new = edit
            .get("newText")
            .or_else(|| edit.get("new_text"))
            .or_else(|| edit.get("new"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !edit.is_object() {
            changes.push_str(&format!(
                "<pre class=\"code edit-raw\">{}</pre>",
                esc(&edit.to_string())
            ));
            continue;
        }
        let lang = badges::guess_lang(name, &format!("{old}\n{new}"));
        changes.push_str(&format!(
            "<section class=\"change\"><div class=\"change-hd\">change {} of {}</div>\
             <div class=\"change-label old\">replace</div>\
             <pre class=\"code edit-old lang-{lang}\">{}</pre>\
             <div class=\"change-label new\">with</div>\
             <pre class=\"code edit-new lang-{lang}\">{}</pre></section>",
            i + 1,
            edits.len(),
            highlight::highlight(old, lang),
            highlight::highlight(new, lang)
        ));
    }
    format!(
        "<div class=\"edit-input\"><div class=\"edit-path\"><span>path</span> <code>{}</code></div>{changes}</div>",
        esc(&path)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::Turn;
    use crate::types::Tool;

    fn session(tool: Tool, turns: Vec<Turn>) -> Session {
        Session {
            id: "sess-1".into(),
            tool,
            cwd: Some("/Users/robertkarl/Code/ubdogs".into()),
            git_branch: Some("main".into()),
            model: Some("test-model".into()),
            name: None,
            started_ms: Some(1_793_400_000_000),
            ended_ms: Some(1_793_400_060_000),
            turns,
            in_tokens: 100,
            out_tokens: 50,
            cost: 0.12,
            unmapped_lines: 0,
        }
    }

    #[test]
    fn a_turn_reaches_both_the_rail_and_the_body() {
        let s = session(
            Tool::Pi,
            vec![
                Turn {
                    role: "user".into(),
                    ts_ms: Some(1_793_400_000_000),
                    text: Some("do the thing".into()),
                    ..Default::default()
                },
                Turn {
                    role: "assistant".into(),
                    ts_ms: Some(1_793_400_005_000),
                    tool_calls: vec![crate::parse::ToolCall {
                        id: Some("c1".into()),
                        name: "Bash".into(),
                        args_json: r#"{"command":"git status | head"}"#.into(),
                        result_text: Some("On branch main".into()),
                        is_error: false,
                    }],
                    ..Default::default()
                },
            ],
        );
        let html = render_session(&s, "debra").unwrap();
        assert_eq!(html.matches("<section class=\"turn").count(), 2);
        assert_eq!(html.matches("<a class=\"tl-row").count(), 2);
        assert!(html.contains("timeline · 2 turns"));
        assert!(html.contains("5.0s"), "per-turn duration in the rail");
        assert!(html.contains("60s"), "wall clock from the index columns");
        assert!(html.contains("🔀"), "git gets its own icon");
        assert!(html.contains("git status | head"), "rail carries the real command");
        assert!(html.contains("ubdogs"), "cwd in the header");
    }

    #[test]
    fn nothing_from_a_transcript_reaches_the_page_unescaped() {
        let s = session(
            Tool::Claude,
            vec![Turn {
                role: "user".into(),
                ts_ms: Some(1_793_400_000_000),
                text: Some("<script>alert(1)</script> & \"quoted\"".into()),
                tool_calls: vec![crate::parse::ToolCall {
                    id: None,
                    name: "Bash".into(),
                    args_json: r#"{"command":"echo </script>"}"#.into(),
                    result_text: Some("</pre></body><script>x</script>".into()),
                    is_error: false,
                }],
                ..Default::default()
            }],
        );
        let html = render_session(&s, "debra").unwrap();
        assert!(!html.contains("<script>alert"), "no injected script in the body");
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        // Exactly the page's own script block, no more.
        assert_eq!(html.matches("<script>").count(), 1);
    }

    #[test]
    fn edits_are_diffs_in_all_three_dialects() {
        // pi: an edits array.
        let pi = session(
            Tool::Pi,
            vec![Turn {
                role: "assistant".into(),
                ts_ms: Some(1_793_400_000_000),
                tool_calls: vec![crate::parse::ToolCall {
                    id: None,
                    name: "edit".into(),
                    args_json: r#"{"path":"/x/y.rs","edits":[{"oldText":"let a = 1;","newText":"let a = 2;"}]}"#.into(),
                    result_text: Some("ok".into()),
                    is_error: false,
                }],
                ..Default::default()
            }],
        );
        let h = render_session(&pi, "debra").unwrap();
        assert!(h.contains("class=\"edit-path\""));
        assert!(h.contains("pre class=\"code edit-old"));
        assert!(h.contains("pre class=\"code edit-new"));
        assert!(h.contains("change 1 of 1"));
        assert!(!h.contains("oldText"), "the transport envelope is not shown");

        // Claude: old_string/new_string, which the original renderer never saw.
        let claude = session(
            Tool::Claude,
            vec![Turn {
                role: "assistant".into(),
                ts_ms: Some(1_793_400_000_000),
                tool_calls: vec![crate::parse::ToolCall {
                    id: None,
                    name: "Edit".into(),
                    args_json: r#"{"file_path":"/x/y.rs","old_string":"fn a()","new_string":"fn b()"}"#.into(),
                    result_text: Some("ok".into()),
                    is_error: false,
                }],
                ..Default::default()
            }],
        );
        let h = render_session(&claude, "debra").unwrap();
        assert!(h.contains("edit-old"));
        assert!(h.contains("fn a()"));
        assert!(h.contains("fn b()"));

        // Codex: a patch as raw text, not JSON at all.
        let codex = session(
            Tool::Codex,
            vec![Turn {
                role: "assistant".into(),
                ts_ms: Some(1_793_400_000_000),
                tool_calls: vec![crate::parse::ToolCall {
                    id: None,
                    name: "apply_patch".into(),
                    args_json: "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-let a = 1;\n+let a = 2;\n*** End Patch".into(),
                    result_text: Some("Success".into()),
                    is_error: false,
                }],
                ..Default::default()
            }],
        );
        let h = render_session(&codex, "debra").unwrap();
        assert!(h.contains("<span class=\"pl-add\">+let a = 2;</span>"));
        assert!(h.contains("<span class=\"pl-del\">-let a = 1;</span>"));
    }

    #[test]
    fn long_output_collapses_short_output_does_not() {
        let big = "x".repeat(1500);
        let s = session(
            Tool::Pi,
            vec![Turn {
                role: "assistant".into(),
                ts_ms: Some(1_793_400_000_000),
                tool_calls: vec![crate::parse::ToolCall {
                    id: None,
                    name: "Bash".into(),
                    args_json: r#"{"command":"cat big"}"#.into(),
                    result_text: Some(big.clone()),
                    is_error: true,
                }],
                ..Default::default()
            }],
        );
        let h = render_session(&s, "debra").unwrap();
        assert!(h.contains("<details class=\"outwrap\"><summary>output · 1 lines · 1500 chars"));
        assert!(h.contains("out err"), "the error is visible in the class");
        assert!(h.contains("⚠️"));

        let small = session(
            Tool::Pi,
            vec![Turn {
                role: "assistant".into(),
                ts_ms: Some(1_793_400_000_000),
                tool_calls: vec![crate::parse::ToolCall {
                    id: None,
                    name: "Bash".into(),
                    args_json: r#"{"command":"ls"}"#.into(),
                    result_text: Some("a\nb".into()),
                    is_error: false,
                }],
                ..Default::default()
            }],
        );
        let h = render_session(&small, "debra").unwrap();
        assert!(h.contains("<div class=\"outwrap\">"));
        assert!(!h.contains("<details class=\"outwrap\">"));
    }

    #[test]
    fn subagent_turns_are_marked_in_rail_and_body() {
        let s = session(
            Tool::Claude,
            vec![
                Turn {
                    role: "assistant".into(),
                    ts_ms: Some(1_793_400_000_000),
                    text: Some("parent".into()),
                    ..Default::default()
                },
                Turn {
                    role: "assistant".into(),
                    ts_ms: Some(1_793_400_001_000),
                    text: Some("child".into()),
                    is_sidechain: true,
                    ..Default::default()
                },
            ],
        );
        let h = render_session(&s, "debra").unwrap();
        assert!(h.contains("class=\"turn asst side\""));
        assert!(h.contains("<span class=\"tside\">subagent</span>"));
        assert!(h.contains("tl-row asst side"));
        assert!(h.contains("subagent turns <b>yes</b>"));
    }

    #[test]
    fn repeated_inputs_are_counted() {
        let call = || crate::parse::ToolCall {
            id: None,
            name: "Bash".into(),
            args_json: r#"{"command":"ls -la"}"#.into(),
            result_text: Some("x".into()),
            is_error: false,
        };
        let s = session(
            Tool::Pi,
            vec![Turn {
                role: "assistant".into(),
                ts_ms: Some(1_793_400_000_000),
                tool_calls: vec![call(), call(), call()],
                ..Default::default()
            }],
        );
        let h = render_session(&s, "debra").unwrap();
        assert!(h.contains("<td class=\"rn\">3×</td>"));
        assert!(h.contains("ls -la"));
        // More than one call in a turn is counted, not just the first.
        assert!(h.contains("<span class=\"tl-more\">+2</span>"));
    }

    #[test]
    fn an_unmodellable_session_says_so() {
        let s = session(Tool::Codex, vec![]);
        let h = render_session(&s, "debra").unwrap();
        assert!(h.contains("no turns the parser could model"));
        assert!(h.contains("timeline · 0 turns"));
    }
}
