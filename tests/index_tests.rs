//! T3 acceptance: blobs -> index. End to end over a throwaway TAPE_HOME, with
//! the invariants that are easy to get wrong:
//!   * a Claude group (main file + subagents file) is ONE session whose turns
//!     from both blobs survive;
//!   * re-ingesting changes nothing;
//!   * an older version of a session never overwrites the newer projection,
//!     but its blob is still recorded;
//!   * turn_count describes the session, not the last file written.

use std::fs;
use std::path::{Path, PathBuf};

use tape::{db, ingest, manifest, paths, store};
use tape::types::{FileEntry, Manifest, Tool};

/// TAPE_HOME is process-global, and cargo runs tests in threads, so every test
/// that sets it must hold this lock for its whole body.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn home(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "tape-t3-{}-{}-{}",
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

fn write_fixture(dir: &Path, name: &str, body: &str) -> FileEntry {
    let path = dir.join("src").join(name);
    fs::write(&path, body).unwrap();
    let bytes = fs::read(&path).unwrap();
    let meta = fs::metadata(&path).unwrap();
    let mtime = meta
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    FileEntry {
        tool: match name {
            n if n.starts_with("pi") => Tool::Pi,
            n if n.starts_with("claude") => Tool::Claude,
            _ => Tool::Codex,
        },
        session_id: match name {
            n if n.starts_with("claude") => "group-1".to_string(),
            n => format!("id-{}", n.trim_end_matches(".jsonl")),
        },
        group: match name {
            n if n.starts_with("claude") => "group-1".to_string(),
            n => format!("id-{}", n.trim_end_matches(".jsonl")),
        },
        path,
        size: bytes.len() as u64,
        mtime_ms: mtime,
        blake3: blake3::hash(&bytes).to_hex().to_string(),
        quiescent: true,
    }
}

const PI: &str = concat!(
    r#"{"type":"session","version":3,"id":"id-pi","timestamp":"2026-04-22T01:23:47.000Z","cwd":"/tmp/x","modelId":"m1"}"#, "\n",
    r#"{"type":"message","message":{"role":"user","content":[{"type":"text","text":"hi"}],"timestamp":1777000000000}}"#, "\n",
    r#"{"type":"message","message":{"role":"assistant","usage":{"input":10,"output":3,"cacheRead":0,"cacheWrite":0,"cost":{"total":0.01}},"timestamp":1777000001000,"content":[{"type":"toolCall","id":"c1","name":"bash","arguments":{"command":"git status | head"}}]}}"#, "\n",
    r#"{"type":"message","message":{"role":"toolResult","toolCallId":"c1","toolName":"bash","isError":false,"timestamp":1777000002000,"content":[{"type":"text","text":"clean"}]}}"#, "\n",
);

const CLAUDE_MAIN: &str = concat!(
    r#"{"type":"mode","sessionId":"group-1"}"#, "\n",
    r#"{"type":"user","isSidechain":false,"cwd":"/tmp/y","gitBranch":"main","timestamp":"2026-07-18T10:00:00.000Z","uuid":"u1","message":{"role":"user","content":"do the thing"}}"#, "\n",
    r#"{"type":"assistant","isSidechain":false,"cwd":"/tmp/y","gitBranch":"main","timestamp":"2026-07-18T10:00:01.000Z","uuid":"a1","message":{"role":"assistant","model":"claude-x","usage":{"input_tokens":5,"cache_read_input_tokens":7,"output_tokens":2},"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la"}}]}}"#, "\n",
    r#"{"type":"user","isSidechain":false,"timestamp":"2026-07-18T10:00:02.000Z","uuid":"u2","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"a file","is_error":false}]}}"#, "\n",
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
    r#"{"timestamp":"2026-03-07T22:40:39.000Z","type":"token_usage_record","payload":{"usage":{"input_tokens":40,"output_tokens":6}}}"#, "\n",
);

struct Fixture {
    home: PathBuf,
    manifest: Manifest,
}

fn fixture(tag: &str) -> Fixture {
    let h = home(tag);
    std::env::set_var("TAPE_HOME", &h);

    let entries = vec![
        write_fixture(&h, "pi.jsonl", PI),
        write_fixture(&h, "claude-main.jsonl", CLAUDE_MAIN),
        write_fixture(&h, "claude-sub.jsonl", CLAUDE_SUB),
        write_fixture(&h, "codex.jsonl", CODEX),
    ];
    // distinct mtimes so the "newest wins" rule is observable
    for (i, e) in entries.iter().enumerate() {
        let t = filetime_set(&e.path, e.mtime_ms + (i as i64) * 1000);
        let _ = t;
    }
    let m = Manifest {
        host: "testhost".into(),
        generated_at: chrono::Utc::now(),
        entries,
    };
    manifest::write_manifest(&m, &paths::manifest_path()).unwrap();

    store::keygen(&paths::identity_path(), &paths::pubkey_path()).unwrap();
    let report = store::store_all(&m.entries, &paths::pubkey_path(), &paths::blobs_dir(), 2)
        .unwrap();
    assert_eq!(report.stored, m.entries.len(), "fixtures must all store");

    Fixture {
        home: h,
        manifest: m,
    }
}

fn filetime_set(path: &Path, ms: i64) -> std::io::Result<()> {
    use std::process::Command;
    // Touching mtime without a crate dependency; fine in a test.
    let secs = (ms / 1000).to_string();
    Command::new("touch")
        .args(["-t", &format_time(&secs), &path.to_string_lossy()])
        .status()
        .map(|_| ())
}

fn format_time(secs: &str) -> String {
    let s: i64 = secs.parse().unwrap_or(0);
    let dt = chrono::DateTime::from_timestamp(s, 0).unwrap_or_default();
    dt.format("%Y%m%d%H%M.%S").to_string()
}

fn q(conn: &rusqlite::Connection, sql: &str) -> Vec<(String, i64)> {
    let mut stmt = conn.prepare(sql).unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

#[test]
fn blobs_project_into_sessions_messages_and_tool_calls() {
    let _g = lock();
    let f = fixture("basic");
    let conn = db::open(&paths::state_db(), "test-index-key").unwrap();
    let r = ingest::ingest(
        &conn,
        &f.manifest,
        &paths::blobs_dir(),
        &paths::identity_path(),
        2,
    )
    .unwrap();

    assert_eq!(r.ingested, 4, "{r:?}");
    assert_eq!(r.parse_failures, 0);
    assert_eq!(r.failures.len(), 0);

    // A Claude group is ONE session even though it is two blobs.
    let rows = q(&conn, "SELECT tool, count(*) FROM sessions GROUP BY tool");
    let map: std::collections::HashMap<String, i64> = rows.into_iter().collect();
    assert_eq!(map.get("claude"), Some(&1), "group collapses to one session");
    assert_eq!(map.get("pi"), Some(&1));
    assert_eq!(map.get("codex"), Some(&1));

    // ...and turns from BOTH claude blobs survive.
    let both: i64 = conn
        .query_row(
            "SELECT count(*) FROM messages m JOIN sessions s ON s.id=m.session_id \
             WHERE s.tool='claude' AND m.blob IN (SELECT hash FROM blobs)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(both >= 4, "main + subagent turns coexist: {both}");
    let sub: i64 = conn
        .query_row(
            "SELECT count(*) FROM messages WHERE role='assistant' AND is_sidechain=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(sub, 2, "subagent turns are marked sidechain");

    // turn_count is a fact about the session, not the last file written.
    let turns: i64 = conn
        .query_row("SELECT turn_count FROM sessions WHERE tool='claude'", [], |r| r.get(0))
        .unwrap();
    assert!(turns >= 4, "{turns}");

    // tool_calls carry a verb derived from the args
    let verbs = q(
        &conn,
        "SELECT verb, count(*) FROM tool_calls WHERE verb IS NOT NULL GROUP BY verb",
    );
    let v: std::collections::HashMap<String, i64> = verbs.into_iter().collect();
    assert!(v.contains_key("git status"), "{v:?}");
    assert!(v.contains_key("main.rs"), "{v:?}"); // Read on /tmp/y/src/main.rs
    assert!(v.contains_key("ls"), "{v:?}");

    // tokens and cost land on the session row
    let (tin, tout, cost): (i64, i64, f64) = conn
        .query_row(
            "SELECT in_tokens, out_tokens, cost FROM sessions WHERE tool='pi'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((tin, tout), (10, 3));
    assert!((cost - 0.01).abs() < 1e-9);

    let blobs = q(&conn, "SELECT tool, count(*) FROM blobs GROUP BY tool");
    let bm: std::collections::HashMap<String, i64> = blobs.into_iter().collect();
    assert_eq!(bm.get("claude"), Some(&2), "both blobs of the group recorded");
}

#[test]
fn reingest_is_a_no_op() {
    let _g = lock();
    let f = fixture("idempotent");
    let conn = db::open(&paths::state_db(), "k").unwrap();
    let a = ingest::ingest(&conn, &f.manifest, &paths::blobs_dir(), &paths::identity_path(), 2)
        .unwrap();
    let before: Vec<(String, i64)> = q(
        &conn,
        "SELECT 'sessions', count(*) FROM sessions UNION ALL
         SELECT 'messages', count(*) FROM messages UNION ALL
         SELECT 'tool_calls', count(*) FROM tool_calls UNION ALL
         SELECT 'blobs', count(*) FROM blobs",
    );
    let b = ingest::ingest(&conn, &f.manifest, &paths::blobs_dir(), &paths::identity_path(), 2)
        .unwrap();
    let after: Vec<(String, i64)> = q(
        &conn,
        "SELECT 'sessions', count(*) FROM sessions UNION ALL
         SELECT 'messages', count(*) FROM messages UNION ALL
         SELECT 'tool_calls', count(*) FROM tool_calls UNION ALL
         SELECT 'blobs', count(*) FROM blobs",
    );
    assert_eq!(a.ingested, 4);
    assert_eq!(b.ingested, 0);
    assert_eq!(b.already_indexed, 4);
    assert_eq!(before, after, "row counts must not move on re-ingest");
}

#[test]
fn an_older_version_never_clobbers_the_newer_projection() {
    let _g = lock();
    let f = fixture("versions");
    let conn = db::open(&paths::state_db(), "k").unwrap();
    ingest::ingest(&conn, &f.manifest, &paths::blobs_dir(), &paths::identity_path(), 2).unwrap();

    // A second, shorter version of the pi session with an OLDER mtime: it is a
    // superseded draft. Its blob must be kept, its projection must not win.
    let old_body = concat!(
        r#"{"type":"session","version":3,"id":"id-pi","timestamp":"2026-04-22T01:23:47.000Z","cwd":"/tmp/x"}"#,
        "\n",
        r#"{"type":"message","message":{"role":"user","content":[{"type":"text","text":"draft"}],"timestamp":1777000000000}}"#,
        "\n",
    );
    let path = f.home.join("src").join("pi-old-draft.jsonl");
    fs::write(&path, old_body).unwrap();
    filetime_set(&path, 1_000_000_000_000).unwrap(); // 2001: unmistakably older
    let bytes = fs::read(&path).unwrap();
    let mut e = f.manifest.entries.iter().find(|x| x.tool == Tool::Pi).unwrap().clone();
    e.path = path;
    e.blake3 = blake3::hash(&bytes).to_hex().to_string();
    e.mtime_ms = 1_000_000_000_000;
    e.size = bytes.len() as u64;
    store::store_blob(&e, &paths::pubkey_path(), &paths::blobs_dir()).unwrap();

    let m2 = Manifest {
        host: "testhost".into(),
        generated_at: chrono::Utc::now(),
        entries: vec![e],
    };
    let r = ingest::ingest(&conn, &m2, &paths::blobs_dir(), &paths::identity_path(), 1).unwrap();
    assert_eq!(r.ingested, 1);

    // Both blobs are recorded; the sessions row still points at the newer one.
    let n: i64 = conn
        .query_row("SELECT count(*) FROM blobs WHERE tool='pi'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 2, "superseded blob stays in Layer 1");
    let text: String = conn
        .query_row(
            "SELECT m.role FROM messages m JOIN sessions s ON s.id=m.session_id \
             WHERE s.tool='pi' AND m.text_len > 0 ORDER BY m.ts_ms LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(text, "user");
    let still_new: i64 = conn
        .query_row(
            "SELECT count(*) FROM sessions s JOIN blobs b ON b.hash=s.blob \
             WHERE s.tool='pi' AND b.source_mtime_ms > 1600000000000",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(still_new, 1, "sessions.blob must still be the newest version");
}

#[test]
fn a_missing_blob_is_reported_not_fatal() {
    let _g = lock();
    let _f = fixture("missing");
    let conn = db::open(&paths::state_db(), "k").unwrap();
    let m = Manifest {
        host: "testhost".into(),
        generated_at: chrono::Utc::now(),
        entries: vec![FileEntry {
            tool: Tool::Pi,
            session_id: "ghost".into(),
            group: "ghost".into(),
            path: PathBuf::from("/nonexistent/ghost.jsonl"),
            size: 10,
            mtime_ms: 1,
            blake3: "0".repeat(64),
            quiescent: true,
        }],
    };
    let r = ingest::ingest(&conn, &m, &paths::blobs_dir(), &paths::identity_path(), 1).unwrap();
    assert_eq!(r.missing_blob, 1);
    assert_eq!(r.ingested, 0);
    assert!(r.failures.is_empty());
}

#[test]
fn the_index_refuses_a_foreign_key() {
    let _g = lock();
    let f = fixture("key");
    let good = db::open(&paths::state_db(), "right").unwrap();
    ingest::ingest(&good, &f.manifest, &paths::blobs_dir(), &paths::identity_path(), 1).unwrap();
    drop(good);
    let err = db::open(&paths::state_db(), "wrong").unwrap_err().to_string();
    assert!(err.contains("wrong key"), "{err}");
}
