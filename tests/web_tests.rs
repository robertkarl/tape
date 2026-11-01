//! T5 acceptance: the browse UI. End to end over a throwaway TAPE_HOME — blobs
//! stored, index built, then pages rendered out of the archive the same way the
//! CT does it. The invariants that matter:
//!   * the list page's counts agree with the index, and every filter narrows
//!     without ever touching SQL text;
//!   * a session page merges a Claude group's two blobs into one session;
//!   * the connection cannot write, so a bug here cannot damage the archive;
//!   * a tampered blob is an error, never a page of mojibake.

use std::fs;
use std::path::{Path, PathBuf};

use tape::render::render_session;
use tape::types::{FileEntry, Manifest, Tool};
use tape::{db, ingest, manifest, paths, store, web};

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn home(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "tape-t5-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    fs::create_dir_all(p.join("src")).unwrap();
    p
}

const PI: &str = concat!(
    r#"{"type":"session","version":3,"id":"id-pi","timestamp":"2026-04-22T01:23:47.000Z","cwd":"/Users/robertkarl/Code/ubdogs","modelId":"m1"}"#, "\n",
    r#"{"type":"message","message":{"role":"user","content":[{"type":"text","text":"hi"}],"timestamp":1777000000000}}"#, "\n",
    r#"{"type":"message","message":{"role":"assistant","usage":{"input":10,"output":3,"cacheRead":0,"cacheWrite":0,"cost":{"total":0.01}},"timestamp":1777000001000,"content":[{"type":"toolCall","id":"c1","name":"bash","arguments":{"command":"git status | head"}},{"type":"toolCall","id":"c2","name":"edit","arguments":{"path":"/Users/robertkarl/Code/ubdogs/app.py","edits":[{"oldText":"x = 1","newText":"x = 2"}]}}]}}"#, "\n",
    r#"{"type":"message","message":{"role":"toolResult","toolCallId":"c1","toolName":"bash","isError":false,"timestamp":1777000002000,"content":[{"type":"text","text":"clean"}]}}"#, "\n",
    r#"{"type":"message","message":{"role":"toolResult","toolCallId":"c2","toolName":"edit","isError":false,"timestamp":1777000003000,"content":[{"type":"text","text":"ok"}]}}"#, "\n",
);

const CLAUDE_MAIN: &str = concat!(
    r#"{"type":"mode","sessionId":"group-1"}"#, "\n",
    r#"{"type":"user","isSidechain":false,"cwd":"/tmp/y","gitBranch":"main","timestamp":"2026-07-18T10:00:00.000Z","uuid":"u1","message":{"role":"user","content":"do the thing"}}"#, "\n",
    r#"{"type":"assistant","isSidechain":false,"cwd":"/tmp/y","gitBranch":"main","timestamp":"2026-07-18T10:00:01.000Z","uuid":"a1","message":{"role":"assistant","model":"claude-x","usage":{"input_tokens":5,"cache_read_input_tokens":7,"output_tokens":2},"content":[{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"/tmp/y/a.rs","old_string":"fn a()","new_string":"fn b()"}}]}}"#, "\n",
    r#"{"type":"user","isSidechain":false,"timestamp":"2026-07-18T10:00:02.000Z","uuid":"u2","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok","is_error":false}]}}"#, "\n",
);

const CLAUDE_SUB: &str = concat!(
    r#"{"type":"assistant","isSidechain":true,"cwd":"/tmp/y","gitBranch":"main","timestamp":"2026-07-18T10:00:03.000Z","uuid":"s1","message":{"role":"assistant","model":"claude-x","content":[{"type":"text","text":"subagent found it"}]}}"#, "\n",
    r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-07-18T10:00:04.000Z","uuid":"s2","message":{"role":"assistant","content":[{"type":"tool_use","id":"t2","name":"Read","input":{"file_path":"/tmp/y/src/main.rs"}}]}}"#, "\n",
);

const CODEX: &str = concat!(
    r#"{"timestamp":"2026-03-07T22:40:34.965Z","type":"session_meta","payload":{"id":"id-codex","cwd":"/tmp/z"}}"#, "\n",
    r#"{"timestamp":"2026-03-07T22:40:35.000Z","type":"turn_context","payload":{"model":"gpt-5.4"}}"#, "\n",
    r#"{"timestamp":"2026-03-07T22:40:36.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"list files"}]}}"#, "\n",
    r#"{"timestamp":"2026-03-07T22:40:37.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"rg --files\"}","call_id":"k1"}}"#, "\n",
    r#"{"timestamp":"2026-03-07T22:40:38.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"k1","output":"main.rs"}}"#, "\n",
);

fn write_fixture(dir: &Path, name: &str, body: &str, tool: Tool, session_id: &str) -> FileEntry {
    let path = dir.join("src").join(name);
    fs::write(&path, body).unwrap();
    let bytes = fs::read(&path).unwrap();
    FileEntry {
        tool,
        session_id: session_id.into(),
        group: session_id.into(),
        path,
        size: bytes.len() as u64,
        mtime_ms: 1_777_000_000_000,
        blake3: blake3::hash(&bytes).to_hex().to_string(),
        quiescent: true,
    }
}

/// A tiny archive: 4 blobs, 3 sessions, one of them a Claude group of two.
fn fixture(tag: &str) -> (PathBuf, Manifest) {
    let h = home(tag);
    std::env::set_var("TAPE_HOME", &h);

    let entries = vec![
        write_fixture(
            &h,
            "pi.jsonl",
            PI,
            Tool::Pi,
            "id-pi",
        ),
        write_fixture(&h, "claude-main.jsonl", CLAUDE_MAIN, Tool::Claude, "group-1"),
        write_fixture(&h, "claude-sub.jsonl", CLAUDE_SUB, Tool::Claude, "group-1"),
        write_fixture(&h, "codex.jsonl", CODEX, Tool::Codex, "id-codex"),
    ];
    let m = Manifest {
        host: "testhost".into(),
        generated_at: chrono::Utc::now(),
        only: None,
        entries,
    };
    manifest::write_manifest(&m, &paths::manifest_path()).unwrap();
    store::keygen(&paths::identity_path(), &paths::pubkey_path()).unwrap();
    let report = store::store_all(&m.entries, &paths::pubkey_path(), &paths::blobs_dir(), 2).unwrap();
    assert_eq!(report.stored, m.entries.len());

    // The same key `web::app_state()` will resolve, so the index it opens is
    // the one ingest wrote.
    let key = db::db_key().unwrap();
    let conn = db::open(&paths::state_db(), &key).unwrap();
    let r = ingest::ingest(
        &conn,
        &m,
        &paths::blobs_dir(),
        &paths::identity_path(),
        2,
    )
    .unwrap();
    let sessions: i64 = conn
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(sessions, 3, "four blobs, three sessions (the Claude group is one)");
    assert!(r.sessions >= 4, "the report counts blob projections");
    drop(conn);
    (h, m)
}

fn rows(html: &str) -> usize {
    html.matches("<tr><td><span class=\"tag").count()
}

#[test]
fn the_list_page_agrees_with_the_index() {
    let _g = lock();
    let (_h, _m) = fixture("list");
    let state = web::app_state().unwrap();

    let all = web::list_html(&state, &web::Filters::default()).unwrap();
    if std::env::var("TAPE_DBG").is_ok() {
        println!("{all}");
    }
    assert!(all.contains("tape · 3 sessions"), "count comes from SQL");
    assert_eq!(rows(&all), 3);
    // The Claude group travels as two blobs; the list says so.
    assert!(all.contains("2 blobs"));
    assert!(all.contains("/s/testhost/group-1"));
    assert!(all.contains("ubdogs"), "cwd is shown");

    // Counts against the index directly, to prove the page is not inventing them.
    let n: i64 = state
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 3);
}

#[test]
fn every_filter_narrows_and_none_of_it_reaches_sql_text() {
    let _g = lock();
    let (_h, _m) = fixture("filters");
    let state = web::app_state().unwrap();

    let mut f = web::Filters::default();
    f.tool = "claude".into();
    let h = web::list_html(&state, &f).unwrap();
    assert_eq!(rows(&h), 1, "one claude session");
    assert!(h.contains("group-1"));
    assert!(!h.contains("id-pi"));

    f = web::Filters::default();
    f.cwd = "ubdogs".into();
    assert_eq!(rows(&web::list_html(&state, &f).unwrap()), 1);

    f = web::Filters::default();
    f.q = "gpt-5.4".into();
    assert_eq!(rows(&web::list_html(&state, &f).unwrap()), 1, "model matches");

    f = web::Filters::default();
    f.host = "some-other-machine".into();
    assert_eq!(rows(&web::list_html(&state, &f).unwrap()), 0);

    // A date range that excludes everything, and one that includes it.
    f = web::Filters::default();
    f.since = "2030-01-01".into();
    assert_eq!(rows(&web::list_html(&state, &f).unwrap()), 0);
    f.since = "2000-01-01".into();
    f.until = "2030-12-31".into();
    assert_eq!(rows(&web::list_html(&state, &f).unwrap()), 3);

    // Injection attempts become parameter values, not SQL.
    f = web::Filters::default();
    f.cwd = "'; DROP TABLE sessions; --".into();
    let h = web::list_html(&state, &f).unwrap();
    assert_eq!(rows(&h), 0, "the filter is a value");
    let n: i64 = state
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 3, "and the table is still there");

    // Garbage in a date field is ignored, not fatal.
    f = web::Filters::default();
    f.since = "not-a-date".into();
    assert_eq!(rows(&web::list_html(&state, &f).unwrap()), 3);
}

#[test]
fn the_web_process_cannot_write_the_archive() {
    let _g = lock();
    let (_h, _m) = fixture("readonly");
    let state = web::app_state().unwrap();
    let conn = state.conn.lock().unwrap();
    let e = conn.execute("INSERT INTO sessions(id,host) VALUES ('x','y')", []);
    assert!(e.is_err(), "query_only must reject the write");
    let n: i64 = conn
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 3);
}

#[test]
fn a_session_page_merges_a_claude_groups_blobs_and_marks_subagents() {
    let _g = lock();
    let (_h, _m) = fixture("merge");
    let state = web::app_state().unwrap();

    let sess = web::load_session(&state, "testhost", "group-1").unwrap();
    assert_eq!(sess.id, "group-1");
    // 2 turns from the main file, 2 from the subagent file.
    assert_eq!(sess.turns.len(), 4, "both blobs contribute turns");
    assert!(
        sess.turns.iter().any(|t| t.is_sidechain),
        "the subagent turns are still marked"
    );

    let html = render_session(&sess, "testhost").unwrap();
    assert_eq!(html.matches("<section class=\"turn").count(), 4);
    assert!(html.contains("<span class=\"tside\">subagent</span>"));
    assert!(html.contains("subagent found it"));
    assert!(html.contains("edit-old"), "Claude edit renders as a diff");
    assert!(html.contains("fn a()") && html.contains("fn b()"));
    assert!(html.contains("📖"), "the subagent's Read gets the book icon");
    assert!(html.contains("<title>claude-x - y - 2026-07-18 - group-1</title>"));
}

#[test]
fn a_pi_page_shows_the_edit_as_a_diff_and_the_bash_as_a_command() {
    let _g = lock();
    let (_h, _m) = fixture("pi");
    let state = web::app_state().unwrap();
    let sess = web::load_session(&state, "testhost", "id-pi").unwrap();
    let html = render_session(&sess, "testhost").unwrap();
    assert!(html.contains("class=\"edit-path\""));
    assert!(html.contains("x = 1") && html.contains("x = 2"));
    assert!(html.contains("git status | head"));
    assert!(html.contains("tok-flag"), "the highlighter ran");
    assert!(html.contains("ubdogs"));
}

#[test]
fn a_tampered_blob_is_an_error_not_a_page() {
    let _g = lock();
    let (_h, m) = fixture("tamper");
    let state = web::app_state().unwrap();

    // Flip one byte inside the ciphertext of one blob.
    let entry = m.entries.iter().find(|e| e.session_id == "id-pi").unwrap();
    let blob = store::blob_path(&paths::blobs_dir(), &entry.blake3);
    let mut bytes = fs::read(&blob).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    fs::write(&blob, &bytes).unwrap();

    let e = web::load_session(&state, "testhost", "id-pi");
    assert!(e.is_err(), "a modified blob must not render");
    let msg = e.err().unwrap().to_string();
    assert!(
        msg.contains("hash mismatch") || msg.contains("decrypt"),
        "and must say why: {msg}"
    );
}

#[test]
fn serve_refuses_an_open_bind() {
    // No auth by decision D3, so binding every interface has to be loud.
    let e = web::serve("0.0.0.0", 8799).unwrap_err();
    assert!(e.to_string().contains("refusing to bind"));
}

#[test]
fn paging_keeps_the_filters() {
    let f = web::Filters {
        tool: "claude".into(),
        cwd: "/tmp/y".into(),
        limit: 10,
        offset: 0,
        ..Default::default()
    };
    let link = web::page_link(&f, 10);
    assert!(link.contains("offset=10"));
    assert!(link.contains("tool=claude"));
    assert!(link.contains("cwd=%2Ftmp%2Fy"), "the value is url-encoded");
}
