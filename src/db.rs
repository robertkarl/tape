//! INDEX — rusqlite with `bundled-sqlcipher` (decision D4).
//!
//! The DDL is PLAN.md §4 **verbatim**. Two additive tables exist beyond it,
//! both index-only and both rebuildable from the blobs:
//!   * `parse_failures` — a blob that decrypted but did not parse is recorded
//!     here so it is visible instead of silently skipped. The blob itself is
//!     never touched.
//!   * `meta`           — schema version bookkeeping (also carried in
//!     `PRAGMA user_version`).
//!
//! `messages.blob` / `tool_calls.blob` are the other addition. A Claude session
//! is several files (the main transcript plus `subagents/agent-*.jsonl`) that
//! all share one `session_id`, so per-turn rows have to be tagged with the blob
//! they came from — otherwise re-ingesting one file deletes the others' turns.
//! `seq` is the turn index *within one file*; cross-file order comes from `ts_ms`.
//!
//! Layer 2 is disposable: `tape ingest` can rebuild every row from Layer 1.
//! Nothing in this module may write outside `TAPE_HOME`.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use rusqlite::Connection;

use crate::paths;

pub const SCHEMA_VERSION: i64 = 1;

/// PLAN.md §4, verbatim, plus the two additive index tables.
pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS hosts   (id TEXT PRIMARY KEY, name TEXT, first_seen INT);
CREATE TABLE IF NOT EXISTS sessions(id TEXT, host TEXT, tool TEXT,
                      cwd TEXT, git_branch TEXT, model TEXT,
                      started_ms INT, ended_ms INT, turn_count INT,
                      in_tokens INT, out_tokens INT, cost REAL,
                      blob TEXT,
                      PRIMARY KEY(id, host));
CREATE TABLE IF NOT EXISTS messages(session_id TEXT, host TEXT, seq INT, role TEXT,
                      ts_ms INT, text_len INT, thinking_len INT,
                      tool_calls INT, is_sidechain INT, blob TEXT);
CREATE TABLE IF NOT EXISTS tool_calls(session_id TEXT, host TEXT, seq INT, name TEXT,
                        verb TEXT, out_len INT, is_error INT, blob TEXT);
CREATE TABLE IF NOT EXISTS blobs(hash TEXT PRIMARY KEY, path TEXT, bytes INT, stored_ms INT,
                   source_path TEXT, source_mtime_ms INT, tool TEXT);
CREATE TABLE IF NOT EXISTS vanished(id TEXT, host TEXT, tool TEXT, last_seen_ms INT);
CREATE TABLE IF NOT EXISTS parse_failures(hash TEXT PRIMARY KEY, source_path TEXT,
                      tool TEXT, error TEXT, at_ms INT);
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT);
CREATE INDEX IF NOT EXISTS ix_sessions_tool ON sessions(tool);
CREATE INDEX IF NOT EXISTS ix_sessions_ended ON sessions(ended_ms DESC);
CREATE INDEX IF NOT EXISTS ix_messages_session ON messages(session_id, host, ts_ms, seq);
CREATE INDEX IF NOT EXISTS ix_messages_blob ON messages(session_id, host, blob);
CREATE INDEX IF NOT EXISTS ix_tool_calls_name ON tool_calls(name);
"#;

/// The index key. Resolution order, first hit wins:
///   1. `TAPE_DB_KEY` environment variable
///   2. `<TAPE_HOME>/db.key` (mode 0400)
///   3. a freshly generated 256-bit key, written to `<TAPE_HOME>/db.key` 0400
///
/// (3) exists so a dev box works without ceremony; on the archive CT the key
/// lives at `/etc/tapes/` and is provided by (1) or (2).
pub fn db_key() -> anyhow::Result<String> {
    if let Ok(k) = std::env::var("TAPE_DB_KEY") {
        if !k.trim().is_empty() {
            return Ok(k);
        }
    }
    let p = paths::db_key_path();
    if let Ok(raw) = fs::read_to_string(&p) {
        let k = raw.trim().to_string();
        if !k.is_empty() {
            return Ok(k);
        }
    }
    let k = random_hex(32).context("generating an index key")?;
    crate::store::atomic_write_bytes(&p, k.as_bytes())?;
    set_mode_0400(&p);
    Ok(k)
}

fn random_hex(nbytes: usize) -> anyhow::Result<String> {
    // No new dependency: the OS CSPRNG is enough for a key we generate locally.
    let mut buf = vec![0u8; nbytes];
    let mut f = fs::File::open("/dev/urandom").context("opening /dev/urandom")?;
    f.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

fn set_mode_0400(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o400));
}

/// Open (or create) the encrypted index and apply the schema.
///
/// A wrong key is detected here rather than at first query: SQLCipher reports
/// "file is not a database" once you touch `sqlite_master`.
pub fn open(path: &Path, key: &str) -> anyhow::Result<Connection> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path)
        .with_context(|| format!("opening index at {}", path.display()))?;
    // PRAGMA key must be the very first statement against the handle.
    let escaped = key.replace('\'', "''");
    conn.execute_batch(&format!("PRAGMA key = '{escaped}';"))
        .context("applying the SQLCipher key")?;
    conn.busy_timeout(Duration::from_secs(15))?;

    // Prove the key actually unlocks the file before touching anything else;
    // any later PRAGMA would fail with a less honest message.
    if let Err(e) = conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(())) {
        anyhow::bail!(
            "cannot open index at {}: wrong key, or the file is not a SQLCipher database ({e})",
            path.display()
        );
    }
    conn.execute_batch("PRAGMA journal_mode = WAL;")?;
    ensure_schema(&conn)?;
    Ok(conn)
}

pub fn open_default() -> anyhow::Result<Connection> {
    open(&paths::state_db(), &db_key()?)
}

pub fn ensure_schema(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(SCHEMA)?;
    let cur: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if cur < SCHEMA_VERSION {
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// small index writers shared by ingest (T3) and audit (T6)
// ---------------------------------------------------------------------------

pub fn upsert_host(conn: &Connection, id: &str, name: &str, now_ms: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO hosts(id, name, first_seen) VALUES (?1, ?2, ?3)",
        rusqlite::params![id, name, now_ms],
    )?;
    Ok(())
}

pub fn known_blob(conn: &Connection, hash: &str) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM blobs WHERE hash = ?1",
        rusqlite::params![hash],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

pub fn record_parse_failure(
    conn: &Connection,
    hash: &str,
    source_path: &str,
    tool: &str,
    error: &str,
    at_ms: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO parse_failures(hash, source_path, tool, error, at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![hash, source_path, tool, error, at_ms],
    )?;
    Ok(())
}

pub fn count_rows(conn: &Connection, table: &str) -> rusqlite::Result<i64> {
    // `table` is never user input; only the names in SCHEMA are passed here.
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_db(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("tape-db-{}-{}.sqlite", tag, std::process::id()));
        let _ = fs::remove_file(&p);
        p
    }

    #[test]
    fn schema_applies_and_roundtrips() {
        let p = tmp_db("schema");
        let conn = open(&p, "hunter2").unwrap();
        for t in [
            "hosts",
            "sessions",
            "messages",
            "tool_calls",
            "blobs",
            "vanished",
            "parse_failures",
        ] {
            assert_eq!(count_rows(&conn, t).unwrap(), 0, "{t} should start empty");
        }
        upsert_host(&conn, "debra", "debra", 1).unwrap();
        upsert_host(&conn, "debra", "debra", 2).unwrap();
        assert_eq!(count_rows(&conn, "hosts").unwrap(), 1);
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
    }

    #[test]
    fn wrong_key_is_rejected() {
        let p = tmp_db("wrongkey");
        {
            let conn = open(&p, "right-key").unwrap();
            conn.execute("INSERT INTO hosts(id,name) VALUES ('h','h')", [])
                .unwrap();
        }
        // Same process, same key: fine.
        assert!(open(&p, "right-key").is_ok());
        // Wrong key must fail loudly, not return an empty database.
        let err = open(&p, "wrong-key").unwrap_err().to_string();
        assert!(err.contains("wrong key"), "{err}");
    }
}

/// Human-readable index summary. Kept deliberately small: it is the instrument
/// that reconciles Layer 2 against `tape scan`'s Layer-1 counts, and it is what
/// T4's acceptance check runs on the archive host.
pub fn stats(conn: &Connection) -> anyhow::Result<String> {
    let mut out = String::new();
    let mut q = |label: &str, sql: &str| -> anyhow::Result<()> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |r| {
            let a: String = r.get(0)?;
            let b: i64 = r.get(1)?;
            Ok((a, b))
        })?;
        out.push_str(label);
        out.push('\n');
        for row in rows {
            let (k, v) = row?;
            out.push_str(&format!("  {k:<44} {v}\n"));
        }
        Ok(())
    };
    q("sessions by tool:", "SELECT tool, count(*) FROM sessions GROUP BY tool ORDER BY 2 DESC")?;
    q("sessions by tool (newest 100 only is not implied):", "SELECT tool, count(DISTINCT id) FROM sessions GROUP BY tool")?;
    q("messages by tool:", "SELECT s.tool, count(*) FROM messages m JOIN sessions s ON s.id = m.session_id AND s.host = m.host GROUP BY s.tool ORDER BY 2 DESC")?;
    q("turns with tool calls:", "SELECT role, count(*) FROM messages WHERE tool_calls > 0 GROUP BY role ORDER BY 2 DESC LIMIT 8")?;
    q("sidechain turns:", "SELECT CAST(is_sidechain AS TEXT), count(*) FROM messages GROUP BY 1")?;
    q("top tool names:", "SELECT name, count(*) FROM tool_calls GROUP BY name ORDER BY 2 DESC LIMIT 12")?;
    q("tool errors:", "SELECT CAST(is_error AS TEXT), count(*) FROM tool_calls GROUP BY 1")?;
    q("blobs by tool:", "SELECT tool, count(*) FROM blobs GROUP BY tool ORDER BY 2 DESC")?;
    q("parse failures:", "SELECT COALESCE(tool,'none'), count(*) FROM parse_failures GROUP BY tool")?;
    q("sessions with no messages:", "SELECT 'yes', count(*) FROM sessions s WHERE NOT EXISTS (SELECT 1 FROM messages m WHERE m.session_id = s.id AND m.host = s.host)")?;
    q("sessions spanning >1 blob:", "SELECT 'yes', count(*) FROM (SELECT session_id, host, count(DISTINCT blob) n FROM messages GROUP BY 1,2 HAVING n > 1)")?;
    Ok(out)
}

/// Print one read-only result set. Used for reconciliation by hand; the web UI
/// (T5) gets its own prepared statements rather than reaching through here.
pub fn dump_query(conn: &Connection, sql: &str) -> anyhow::Result<()> {
    let mut stmt = conn.prepare(sql)?;
    let n = stmt.column_count();
    let names: Vec<String> = stmt.column_names().map(|s| s.to_string()).collect();
    println!("{}", names.join("\t"));
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let mut cells = Vec::with_capacity(n);
        for i in 0..n {
            let v: rusqlite::types::Value = r.get(i)?;
            cells.push(match v {
                rusqlite::types::Value::Null => String::new(),
                rusqlite::types::Value::Integer(i) => i.to_string(),
                rusqlite::types::Value::Real(f) => format!("{f}"),
                rusqlite::types::Value::Text(t) => t,
                rusqlite::types::Value::Blob(b) => format!("<{} bytes>", b.len()),
            });
        }
        println!("{}", cells.join("\t"));
    }
    Ok(())
}

/// Guard the ad-hoc query surface: one statement, read-only.
pub fn readonly_sql(q: &str) -> anyhow::Result<String> {
    let trimmed = q.trim().trim_end_matches(';').trim();
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("select") || lower.starts_with("with") || lower.starts_with("pragma"))
        || lower.contains(';')
    {
        anyhow::bail!("--sql accepts one read-only SELECT/WITH/PRAGMA statement");
    }
    Ok(trimmed.to_string())
}
