//! INGEST — Layer 1 (blobs) -> Layer 2 (index).
//!
//! Direction matters: blobs are the truth, the index is a projection. So
//!   * a blob is never rewritten, and never deleted here;
//!   * re-ingesting the same hash is a no-op (idempotent);
//!   * an older version of a session never overwrites a newer one — the
//!     superseded blob stays, the sessions row keeps the newest;
//!   * a parse failure is *recorded* (`parse_failures`) and the run continues.
//!     Losing an index row must never cost the archive a byte of transcript.
//!
//! Decrypt + decompress + parse run in parallel; SQLite writes run serially in
//! batches, which both keeps rusqlite single-threaded and bounds peak memory
//! (the archive CT gets 1 GB).

use std::fs;
use std::path::Path;
use std::thread;

use anyhow::Context;
use rusqlite::{params, Connection};

use crate::db;
use crate::parse::{self, Session};
use crate::paths;
use crate::store;
use crate::types::{Manifest, Tool};

#[derive(Debug, Default)]
pub struct IngestReport {
    pub manifest_entries: usize,
    pub already_indexed: usize,
    pub missing_blob: usize,
    pub ingested: usize,
    pub parse_failures: usize,
    pub sessions: usize,
    pub messages: usize,
    pub tool_calls: usize,
    /// Blobs that could not be read or decrypted (distinct from parse failures).
    pub failures: Vec<String>,
}

/// How many manifest entries a single batch decrypts+parses at once.
const BATCH_FACTOR: usize = 4;

/// Ingest every manifest tape knows about — this host's own plus the per-host
/// manifests that `tape push` delivered. On the archive CT that is one per
/// source machine, which is what keeps two machines' sessions apart (`host` is
/// part of the `sessions`/`messages` key).
pub fn ingest_all(
    conn: &Connection,
    manifests: &[Manifest],
    blobs_dir: &Path,
    identity_path: &Path,
    jobs: usize,
) -> anyhow::Result<IngestReport> {
    let mut total = IngestReport::default();
    for m in manifests {
        let r = ingest(conn, m, blobs_dir, identity_path, jobs)?;
        total.manifest_entries += r.manifest_entries;
        total.already_indexed += r.already_indexed;
        total.missing_blob += r.missing_blob;
        total.ingested += r.ingested;
        total.parse_failures += r.parse_failures;
        total.sessions += r.sessions;
        total.messages += r.messages;
        total.tool_calls += r.tool_calls;
        total.failures.extend(r.failures);
    }
    Ok(total)
}

struct Parsed {
    entry_idx: usize,
    session: Session,
}

pub fn ingest(
    conn: &Connection,
    manifest: &Manifest,
    blobs_dir: &Path,
    identity_path: &Path,
    jobs: usize,
) -> anyhow::Result<IngestReport> {
    let identity = store::load_identity(identity_path)
        .with_context(|| format!("loading identity {}", identity_path.display()))?;
    let host = manifest.host.clone();
    let now_ms = store::now_ms();

    db::upsert_host(conn, &host, &host, now_ms)?;

    let mut report = IngestReport {
        manifest_entries: manifest.entries.len(),
        ..Default::default()
    };

    // Only entries that actually have a blob here, and that we have not seen.
    let mut todo: Vec<usize> = Vec::new();
    for (i, e) in manifest.entries.iter().enumerate() {
        if !e.quiescent {
            continue;
        }
        if !store::blob_path(blobs_dir, &e.blake3).exists() {
            report.missing_blob += 1;
            continue;
        }
        if db::known_blob(conn, &e.blake3)? {
            report.already_indexed += 1;
            continue;
        }
        todo.push(i);
    }

    let jobs = jobs.max(1);
    for batch in todo.chunks(jobs * BATCH_FACTOR) {
        // ---- parallel: read, decrypt, decompress, parse --------------------
        let mut results: Vec<Result<Parsed, (usize, String)>> = Vec::new();
        let chunks: Vec<&[usize]> = split(batch, jobs);
        thread::scope(|sc| {
            let mut handles = Vec::new();
            for chunk in chunks {
                let ident = &identity;
                let entries = &manifest.entries;
                let dir = blobs_dir;
                handles.push(sc.spawn(move || {
                    let mut out: Vec<Result<Parsed, (usize, String)>> = Vec::new();
                    for &idx in chunk {
                        let e = &entries[idx];
                        let path = store::blob_path(dir, &e.blake3);
                        out.push(match decode(&path, ident, &e.blake3) {
                            Err(why) => Err((idx, format!("{}: {why}", path.display()))),
                            Ok(text) => match parse::parse_for(e.tool, &text) {
                                Ok(session) => Ok(Parsed {
                                    entry_idx: idx,
                                    session,
                                }),
                                Err(why) => Err((idx, format!("parse {}: {why}", e.path.display()))),
                            },
                        });
                    }
                    out
                }));
            }
            for h in handles {
                if let Ok(part) = h.join() {
                    results.extend(part);
                }
            }
        });

        // ---- serial: one transaction per batch ----------------------------
        results.sort_by_key(|r| r.as_ref().map(|p| p.entry_idx).unwrap_or(usize::MAX));
        let tx = conn.unchecked_transaction()?;
        for r in results {
            match r {
                Ok(p) => {
                    let e = &manifest.entries[p.entry_idx];
                    let n = write_session(&tx, &host, e, &p.session, now_ms)?;
                    report.sessions += n.0;
                    report.messages += n.1;
                    report.tool_calls += n.2;
                    report.ingested += 1;
                }
                Err((idx, why)) => {
                    let e = &manifest.entries[idx];
                    if why.starts_with("parse ") {
                        report.parse_failures += 1;
                        db::record_parse_failure(
                            &tx,
                            &e.blake3,
                            &e.path.to_string_lossy(),
                            e.tool.as_str(),
                            &why,
                            now_ms,
                        )?;
                    } else {
                        report.failures.push(why);
                    }
                }
            }
        }
        tx.commit()?;
    }

    Ok(report)
}

fn decode(path: &Path, identity: &age::x25519::Identity, expect_hash: &str) -> anyhow::Result<String> {
    let ciphertext = fs::read(path).context("reading blob")?;
    let plaintext = store::open_and_check(&ciphertext, identity, expect_hash)?;
    Ok(String::from_utf8_lossy(&plaintext).into_owned())
}

/// Insert one session's projection. Returns (sessions, messages, tool_calls).
fn write_session(
    conn: &Connection,
    host: &str,
    entry: &crate::types::FileEntry,
    s: &Session,
    now_ms: i64,
) -> anyhow::Result<(usize, usize, usize)> {
    let blob_path = store::blob_path(&paths::blobs_dir(), &entry.blake3);
    let bytes = fs::metadata(&blob_path).map(|m| m.len()).unwrap_or(0);

    conn.execute(
        "INSERT OR REPLACE INTO blobs(hash, path, bytes, stored_ms, source_path, source_mtime_ms, tool) \
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            entry.blake3,
            blob_path.to_string_lossy().into_owned(),
            bytes as i64,
            now_ms,
            entry.path.to_string_lossy().into_owned(),
            entry.mtime_ms,
            entry.tool.as_str(),
        ],
    )?;

    // Turn rows are per blob, so a group (Claude main + subagents) coexists in
    // one session, and re-ingesting one blob replaces only that blob's rows.
    conn.execute(
        "DELETE FROM messages WHERE session_id = ?1 AND host = ?2 AND blob = ?3",
        params![entry.session_id, host, entry.blake3],
    )?;
    conn.execute(
        "DELETE FROM tool_calls WHERE session_id = ?1 AND host = ?2 AND blob = ?3",
        params![entry.session_id, host, entry.blake3],
    )?;

    let mut msgs = 0usize;
    let mut calls = 0usize;
    for (seq, t) in s.turns.iter().enumerate() {
        conn.execute(
            "INSERT INTO messages(session_id, host, seq, role, ts_ms, text_len, thinking_len, \
               tool_calls, is_sidechain, blob) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                entry.session_id,
                host,
                seq as i64,
                t.role,
                t.ts_ms,
                t.text.as_ref().map(|x| x.chars().count() as i64),
                t.thinking.as_ref().map(|x| x.chars().count() as i64),
                t.tool_calls.len() as i64,
                t.is_sidechain as i64,
                entry.blake3,
            ],
        )?;
        msgs += 1;
        for tc in &t.tool_calls {
            conn.execute(
                "INSERT INTO tool_calls(session_id, host, seq, name, verb, out_len, is_error, blob) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    entry.session_id,
                    host,
                    seq as i64,
                    tc.name,
                    parse::verb_for(&tc.name, &tc.args_json),
                    tc.result_text.as_ref().map(|x| x.chars().count() as i64),
                    tc.is_error as i64,
                    entry.blake3,
                ],
            )?;
            calls += 1;
        }
    }

    // A superseded version stays in Layer 1 but must not clobber the newer
    // projection: only the newest blob for a session owns the sessions row.
    let prev: Option<i64> = conn
        .query_row(
            "SELECT b.source_mtime_ms FROM sessions s JOIN blobs b ON b.hash = s.blob \
             WHERE s.id = ?1 AND s.host = ?2",
            params![entry.session_id, host],
            |r| r.get(0),
        )
        .ok();
    if prev.is_some_and(|p| p > entry.mtime_ms) {
        recompute_turn_count(conn, &entry.session_id, host)?;
        return Ok((0, msgs, calls));
    }

    conn.execute(
        "INSERT OR REPLACE INTO sessions(id, host, tool, cwd, git_branch, model, \
           started_ms, ended_ms, turn_count, in_tokens, out_tokens, cost, blob) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        params![
            entry.session_id,
            host,
            entry.tool.as_str(),
            s.cwd,
            s.git_branch,
            s.model,
            s.started_ms,
            s.ended_ms.or(Some(entry.mtime_ms)),
            s.turns.len() as i64,
            s.in_tokens,
            s.out_tokens,
            s.cost,
            entry.blake3,
        ],
    )?;
    recompute_turn_count(conn, &entry.session_id, host)?;

    Ok((1, msgs, calls))
}

/// A session spans several files, so turn_count is a fact about the whole
/// session, not about the file that happened to be written last.
fn recompute_turn_count(conn: &Connection, session_id: &str, host: &str) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE sessions SET turn_count = \
           (SELECT count(*) FROM messages WHERE session_id = ?1 AND host = ?2) \
         WHERE id = ?1 AND host = ?2",
        params![session_id, host],
    )?;
    Ok(())
}

fn split<T>(v: &[T], parts: usize) -> Vec<&[T]> {
    if parts <= 1 {
        return vec![v];
    }
    let per = (v.len() + parts - 1) / parts;
    v.chunks(per.max(1)).collect()
}

/// Convenience for the CLI: index everything the manifest references.
pub fn ingest_default_store(
    conn: &Connection,
    manifest: &Manifest,
    jobs: usize,
) -> anyhow::Result<IngestReport> {
    ingest(conn, manifest, &paths::blobs_dir(), &paths::identity_path(), jobs)
}

/// Tool of the newest blob for a session, for reporting.
pub fn tool_of_blob(conn: &Connection, hash: &str) -> anyhow::Result<Option<Tool>> {
    let t: Option<String> = conn
        .query_row(
            "SELECT tool FROM blobs WHERE hash = ?1",
            params![hash],
            |r| r.get(0),
        )
        .ok();
    Ok(match t.as_deref() {
        Some("pi") => Some(Tool::Pi),
        Some("claude") => Some(Tool::Claude),
        Some("codex") => Some(Tool::Codex),
        _ => None,
    })
}
