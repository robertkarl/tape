//! SOURCE DISCOVERY — owned by T1.
//!
//! Walk the three tools' transcript roots, group Claude subagent files with
//! their parent session, hash, and apply the quiescence window.
//! See TASKS.md T1 for acceptance criteria.
//!
//! Nothing here writes anything. The only writes in T1 are in
//! `manifest::write_manifest`, which is confined to `paths::manifest_path()`.

use std::fs::File;
use std::io::BufRead;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use chrono::Utc;
use serde_json::Value;
use walkdir::WalkDir;

use crate::paths;

// The shared types so integration tests (tests/scan_test.rs) can
// reach them without touching main.rs or types.rs.
pub use crate::types::{FileEntry, Manifest, Tool};

/// Minutes a file must be untouched before we archive it. Transcript files are
/// appended to while a session runs; a partial tail is worse than a late copy.
pub const QUIESCENCE_MINUTES: u64 = 10;

/// Home-directory-rooted source roots, expanded at runtime.
pub fn source_roots() -> Vec<(Tool, PathBuf)> {
    let home = dirs_home();
    vec![
        (Tool::Claude, PathBuf::from(&home).join(".claude/projects")),
        (Tool::Codex, PathBuf::from(&home).join(".codex/sessions")),
        (Tool::Codex, PathBuf::from(&home).join(".codex/archived_sessions")),
        (Tool::Pi, PathBuf::from(&home).join(".pi/agent/sessions")),
    ]
}

fn dirs_home() -> String {
    std::env::var("HOME").expect("HOME unset")
}

/// scan() + the anomalies found on the way (files that broke an assumption).
/// Anomalies are reported, never dropped: a mismatch is a finding.
pub struct ScanResult {
    pub manifest: Manifest,
    pub anomalies: Vec<String>,
}

/// T1: produce the manifest, persist it atomically under TAPE_HOME, and
/// report anomalies. Writes nothing outside `paths::tape_home()`.
///
/// Keeps the scaffolded `main.rs` contract (`scan() -> Manifest`). The
/// human-readable per-tool summary is emitted via `tracing::info!` so it lands
/// on stderr and `tape scan --json` stays pipeable on stdout.
pub fn scan() -> anyhow::Result<Manifest> {
    let result = scan_with(&source_roots(), now_ms(), QUIESCENCE_MINUTES)?;
    for a in &result.anomalies {
        tracing::warn!("anomaly: {a}");
    }
    tracing::info!("\n{}", summarize(&result));
    let manifest = result.manifest;
    crate::manifest::write_manifest(&manifest, &paths::manifest_path())?;
    Ok(manifest)
}

/// The testable core: roots, "now" (ms since epoch), and the quiescence
/// window are all injectable so tests can fabricate fixtures in /tmp.
pub fn scan_with(
    roots: &[(Tool, PathBuf)],
    now_ms: i64,
    quiescence_minutes: u64,
) -> anyhow::Result<ScanResult> {
    let quiesce_before_ms = now_ms - (quiescence_minutes as i64) * 60_000;
    let mut entries: Vec<FileEntry> = Vec::new();
    let mut anomalies: Vec<String> = Vec::new();

    for (tool, root) in roots {
        if !root.exists() {
            anomalies.push(format!("source root missing: {}", root.display()));
            continue;
        }
        for entry in WalkDir::new(root).sort_by_file_name().into_iter() {
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    anomalies.push(format!("walkdir error: {err}"));
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let (fe, note) = classify_and_hash(*tool, root, path, quiesce_before_ms)?;
            if let Some(note) = note {
                anomalies.push(format!("{note} (session_id={}, group={})", fe.session_id, fe.group));
            }
            entries.push(fe);
        }
    }

    entries.sort_by(|a, b| {
        (&a.tool, &a.group, &a.path).cmp(&(&b.tool, &b.group, &b.path))
    });

    Ok(ScanResult {
        manifest: Manifest {
            host: hostname(),
            generated_at: Utc::now(),
            entries,
        },
        anomalies,
    })
}

/// Classify one .jsonl file by (tool, position under its root), stat it, and
/// blake3 its exact bytes. Returns the entry plus an optional anomaly note:
/// a file that broke an assumption still lands in the manifest and is still
/// reported — a mismatch is a finding, never something to filter away. An
/// Err is only for a file we cannot stat or read at all.
fn classify_and_hash(
    tool: Tool,
    root: &Path,
    path: &Path,
    quiesce_before_ms: i64,
) -> anyhow::Result<(FileEntry, Option<String>)> {
    let meta = std::fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?;
    let mtime_ms = meta
        .modified()
        .map(|t| t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0))
        .unwrap_or(0);
    let quiescent = mtime_ms <= quiesce_before_ms;

    let (session_id, group, note) = match tool {
        Tool::Claude => classify_claude(root, path),
        Tool::Codex => classify_codex(path)?,
        Tool::Pi => classify_pi(path)?,
    };

    let blake3 = hash_file(path)?;

    let mut note = note;
    if meta.len() == 0 {
        note = Some(format!("{}: zero-byte file", path.display()));
    }

    Ok((
        FileEntry {
            tool,
            session_id,
            group,
            path: path.to_path_buf(),
            size: meta.len(),
            mtime_ms,
            blake3,
            quiescent,
        },
        note,
    ))
}

/// Claude layout: <enc-cwd>/<uuid>.jsonl, plus <uuid>/subagents/agent-*.jsonl
/// which must carry the PARENT's id and group so the set archives together.
///
/// Measured on debra 2026-10-31, exactly two shapes appear under the root:
///   <enc-cwd>/<uuid>.jsonl                    (99 top-level sessions)
///   <enc-cwd>/<uuid>/subagents/agent-*.jsonl  (106 subagent files)
/// Anything else is a finding, not something to absorb silently.
fn classify_claude(root: &Path, path: &Path) -> (String, String, Option<String>) {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let comps: Vec<&str> = rel.components().map(Component::as_os_str).filter_map(|s| s.to_str()).collect();
    let file_name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let stem = file_name.strip_suffix(".jsonl").unwrap_or(file_name);

    match comps.len() {
        // <enc-cwd>/<uuid>.jsonl — the top-level session file.
        2 => {
            let id = stem.to_string();
            let note = if looks_like_uuid(&id) {
                None
            } else {
                Some(format!("{}: session file stem is not a uuid", path.display()))
            };
            (id.clone(), id, note)
        }
        // <enc-cwd>/<uuid>/subagents/agent-*.jsonl — rides with the parent.
        4 if comps[2] == "subagents" => {
            let parent_uuid = comps[1].to_string();
            if !looks_like_uuid(&parent_uuid) {
                return (
                    stem.to_string(),
                    parent_uuid,
                    Some(format!("{}: subagents dir is not named for a session uuid", path.display())),
                );
            }
            // An orphaned subagents dir (no sibling <uuid>.jsonl) is a finding.
            let session_file = path
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.parent())
                .map(|cwd| cwd.join(format!("{parent_uuid}.jsonl")));
            if let Some(session_file) = session_file {
                if !session_file.exists() {
                    return (
                        parent_uuid.clone(),
                        parent_uuid,
                        Some(format!(
                            "{}: orphaned subagent file (no {})",
                            path.display(),
                            session_file.display()
                        )),
                    );
                }
            }
            (parent_uuid.clone(), parent_uuid, None)
        }
        // Any other shape: keep the file (grouped by the uuid we can see) and report.
        _ => {
            let guess = comps
                .iter()
                .find(|c| looks_like_uuid(c))
                .map(|s| s.to_string())
                .unwrap_or_else(|| stem.to_string());
            (
                guess.clone(),
                guess,
                Some(format!("{}: unexpected Claude layout", path.display())),
            )
        }
    }
}

/// Codex: rollout-<date>T<time>-<uuid>.jsonl. Prefer session_meta.payload.id
/// from the first line; fall back to the trailing uuid in the filename.
/// Returns (session_id, group, anomaly).
fn classify_codex(path: &Path) -> anyhow::Result<(String, String, Option<String>)> {
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let stem = file_name.strip_suffix(".jsonl").unwrap_or(file_name);
    // rollout-<date>T<time>-<uuid>: the uuid is the trailing 36 chars.
    let tail = stem.chars().rev().take(36).collect::<Vec<_>>().into_iter().rev().collect::<String>();

    let from_line = first_json_line(path, 16 * 1024 * 1024)?.and_then(|v| {
        if v.get("type").and_then(Value::as_str) == Some("session_meta") {
            v.pointer("/payload/id").and_then(Value::as_str).map(String::from)
        } else {
            None
        }
    });

    let (id, note) = match from_line {
        Some(id) => {
            if !looks_like_uuid(&id) {
                (id, Some(format!("{}: session_meta id is not a uuid", path.display())))
            } else if !tail.is_empty() && tail != id {
                (
                    id.clone(),
                    Some(format!("{}: filename tail {tail} != session_meta id {id}", path.display())),
                )
            } else {
                (id, None)
            }
        }
        None => (
            tail,
            Some(format!("{}: no usable session_meta id; fell back to filename", path.display())),
        ),
    };
    Ok((id.clone(), id, note))
}

/// pi: <enc-cwd>/<ISO-ts>_<id8>.jsonl. The authoritative id is the first
/// line's {"type":"session","id":"<uuid>"}. Returns (session_id, group, anomaly).
fn classify_pi(path: &Path) -> anyhow::Result<(String, String, Option<String>)> {
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let stem = file_name.strip_suffix(".jsonl").unwrap_or(file_name);
    let fallback = stem.rsplit('_').next().unwrap_or(stem).to_string();

    let from_line = first_json_line(path, 1024 * 1024)?.and_then(|v| {
        if v.get("type").and_then(Value::as_str) == Some("session") {
            v.get("id").and_then(Value::as_str).map(String::from)
        } else {
            None
        }
    });

    let (id, note) = match from_line {
        Some(id) => (id, None),
        None => (
            format!("unparsed-{fallback}"),
            Some(format!("{}: first line is not a pi session header; fell back to filename id8", path.display())),
        ),
    };
    Ok((id.clone(), id, note))
}

/// Read the first line of `path` and parse it as JSON. None if the file is
/// empty, the line exceeds `max_bytes`, or it is not valid JSON.
fn first_json_line(path: &Path, max_bytes: usize) -> anyhow::Result<Option<Value>> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(err) => return Err(anyhow::Error::new(err).context(format!("open {}", path.display()))),
    };
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();
    let read = reader
        .read_line(&mut line)
        .with_context(|| format!("read first line of {}", path.display()))?;
    if read == 0 || line.trim().is_empty() {
        return Ok(None);
    }
    if line.len() > max_bytes {
        return Ok(None);
    }
    Ok(serde_json::from_str(line.trim()).ok())
}

fn hash_file(path: &Path) -> anyhow::Result<String> {
    let file = File::open(path)
        .with_context(|| format!("open {}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = blake3::Hasher::new();
    hasher
        .update_reader(&mut reader)
        .with_context(|| format!("blake3 {}", path.display()))?;
    Ok(hasher.finalize().to_hex().to_string())
}

fn looks_like_uuid(s: &str) -> bool {
    let cs: Vec<char> = s.chars().collect();
    if cs.len() != 36 {
        return false;
    }
    let is_hex = |c: char| c.is_ascii_hexdigit();
    let hyphen_at = |i: usize| cs[i] == '-';
    let hex_runs = [(0usize, 8), (9, 4), (14, 4), (19, 4), (24, 12)];
    hyphen_at(8) && hyphen_at(13) && hyphen_at(18) && hyphen_at(23)
        && hex_runs.iter().all(|(start, len)| {
            (0..*len).all(|k| is_hex(cs[start + k]))
        })
}

/// The host this scan ran on. macOS-first (debra): scutil's ComputerName,
/// falling back to `uname -n`.
fn hostname() -> String {
    if let Ok(out) = Command::new("scutil").args(["--get", "ComputerName"]).output() {
        if out.status.success() {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !name.is_empty() {
                return name;
            }
        }
    }
    if let Ok(out) = Command::new("uname").arg("-n").output() {
        if out.status.success() {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !name.is_empty() {
                return name;
            }
        }
    }
    "unknown".to_string()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Human-readable summary for `tape scan`: per-tool file counts and bytes,
/// Claude groups carrying subagents, and the non-quiescent count.
pub fn summarize(result: &ScanResult) -> String {
    let m = &result.manifest;
    let mut out = String::new();
    out.push_str(&format!("host: {}  generated: {}\n", m.host, m.generated_at.to_rfc3339()));

    let mut counts: Vec<(Tool, u64, u64)> = vec![
        (Tool::Pi, 0, 0),
        (Tool::Claude, 0, 0),
        (Tool::Codex, 0, 0),
    ];
    for e in &m.entries {
        let row = counts.iter_mut().find(|(t, ..)| *t == e.tool).unwrap();
        row.1 += 1;
        row.2 += e.size;
    }
    for (tool, n, bytes) in &counts {
        out.push_str(&format!("{:<6} {:>5} files  {:>12} bytes\n", tool.as_str(), n, bytes));
    }

    let mut with_sub: std::collections::HashSet<&str> = Default::default();
    let mut with_main: std::collections::HashSet<&str> = Default::default();
    for e in &m.entries {
        if e.tool != Tool::Claude {
            continue;
        }
        if is_claude_subagent(&e.path) {
            with_sub.insert(e.group.as_str());
        } else {
            with_main.insert(e.group.as_str());
        }
    }
    let claude_subagent_groups = with_sub.intersection(&with_main).count();
    out.push_str(&format!("claude groups with subagents: {claude_subagent_groups}\n"));

    let non_quiescent = m.entries.iter().filter(|e| !e.quiescent).count();
    out.push_str(&format!("non-quiescent files: {non_quiescent}\n"));
    out.push_str(&format!("total: {} files\n", m.entries.len()));

    if !result.anomalies.is_empty() {
        out.push_str(&format!("anomalies: {}\n", result.anomalies.len()));
        for a in &result.anomalies {
            out.push_str(&format!("  ! {a}\n"));
        }
    }
    out
}

/// True if a Claude entry is a subagent file (<uuid>/subagents/agent-*.jsonl).
fn is_claude_subagent(path: &Path) -> bool {
    path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()) == Some("subagents")
}

#[cfg(test)]
mod tests {
    //! Fabricated source trees in /tmp, scanned through the real pipeline.
    //!
    //! House rule: no `rm`, ever — scratch dirs are timestamped under /tmp and
    //! left behind; each is a few KB.
    use super::*;
    use std::fs;

    const CLAUDE_UUID: &str = "6f1d2e10-1111-4aaa-8bbb-0000000000c1";
    const CLAUDE_ORPHAN: &str = "6f1d2e10-2222-4aaa-8bbb-0000000000c2";
    const CODEX_UUID: &str = "019cca75-aa94-7a50-a97b-8e8e35341ae9";
    const PI_UUID: &str = "57065825-2433-4e0a-b776-86bbb4174408";

    struct Fixtures {
        roots: Vec<(Tool, PathBuf)>,
        now_ms: i64,
    }

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn pin_mtime(path: &Path, secs: u64) {
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_times(
            fs::FileTimes::new().set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs)),
        )
        .unwrap();
    }

    fn fixtures() -> Fixtures {
        let nanos = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = PathBuf::from(format!("/tmp/tape-t1-unit-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&root).unwrap();

        let now = std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let old = (now - 3600) as u64; // outside the 10-min window
        let recent = (now - 60) as u64; // inside it

        // Claude: <enc-cwd>/<uuid>.jsonl + <uuid>/subagents/agent-*.jsonl.
        let cwd_dir = root.join("claude/projects").join("-Users-test-Code-x");
        let main_file = cwd_dir.join(format!("{CLAUDE_UUID}.jsonl"));
        let sub1 = cwd_dir.join(CLAUDE_UUID).join("subagents").join("agent-a1.jsonl");
        let sub2 = cwd_dir.join(CLAUDE_UUID).join("subagents").join("agent-a2.jsonl");
        // An orphaned subagents dir: no sibling <uuid>.jsonl.
        let orphan = cwd_dir.join(CLAUDE_ORPHAN).join("subagents").join("agent-a3.jsonl");
        write(&main_file, "{\"type\":\"mode\",\"mode\":\"normal\"}\n");
        write(&sub1, "{\"type\":\"assistant\"}\n");
        write(&sub2, "{\"type\":\"assistant\"}\n");
        write(&orphan, "{\"type\":\"assistant\"}\n");

        // Codex: good session_meta line, broken first line, zero-byte file.
        let codex_dir = root.join("codex/sessions/2026/03/07");
        let codex_good = codex_dir.join(format!("rollout-2026-03-07T14-40-34-{CODEX_UUID}.jsonl"));
        let codex_broken_name = codex_dir.join(format!("rollout-2026-03-08T10-00-00-{CLAUDE_UUID}.jsonl"));
        let codex_empty = root.join("codex/archived_sessions").join("rollout-2026-04-06T18-02-15-broken.jsonl");
        write(
            &codex_good,
            "{\"timestamp\":\"x\",\"type\":\"session_meta\",\"payload\":{\"id\":\"019cca75-aa94-7a50-a97b-8e8e35341ae9\"}}\n",
        );
        write(&codex_broken_name, "not json at all\n");
        write(&codex_empty, "");

        // pi: the authoritative id is in the first line, not the filename.
        let pi_dir = root.join("pi/sessions/--private-tmp--");
        let pi_good = pi_dir.join("2026-07-22T23-55-18.914Z_57065825.jsonl");
        let inflight = pi_dir.join("2026-10-31T00-00-00.000Z_aaaaaaaa.jsonl");
        write(&pi_good, &format!("{{\"type\":\"session\",\"version\":3,\"id\":\"{PI_UUID}\"}}\n"));
        write(
            &inflight,
            "{\"type\":\"session\",\"version\":3,\"id\":\"aaaaaaaa-0000-4000-8000-000000000000\"}\n",
        );
        pin_mtime(&inflight, recent);

        // A non-jsonl sibling that must be skipped.
        write(&pi_dir.join("sidecar.txt"), "ignore me");

        let mut old_files = vec![main_file, sub1, sub2, orphan, codex_good, codex_broken_name, codex_empty, pi_good];
        old_files.sort();
        for p in old_files {
            pin_mtime(&p, old);
        }

        Fixtures {
            roots: vec![
                (Tool::Claude, root.join("claude/projects")),
                (Tool::Codex, root.join("codex/sessions")),
                (Tool::Codex, root.join("codex/archived_sessions")),
                (Tool::Pi, root.join("pi/sessions")),
            ],
            now_ms: now * 1000,
        }
    }

    fn scanned() -> ScanResult {
        let f = fixtures();
        scan_with(&f.roots, f.now_ms, QUIESCENCE_MINUTES).unwrap()
    }

    fn find<'a>(r: &'a ScanResult, needle: &str) -> &'a FileEntry {
        r.manifest
            .entries
            .iter()
            .find(|e| e.path.to_string_lossy().contains(needle))
            .unwrap_or_else(|| panic!("no entry matching {needle}"))
    }

    #[test]
    fn claude_subagents_carry_the_parents_id_and_group() {
        let r = scanned();
        let main = find(&r, &format!("{CLAUDE_UUID}.jsonl"));
        assert_eq!(main.session_id, CLAUDE_UUID);
        assert_eq!(main.group, CLAUDE_UUID);

        let subs: Vec<_> = r
            .manifest
            .entries
            .iter()
            .filter(|e| e.tool == Tool::Claude && is_claude_subagent(&e.path) && e.group == CLAUDE_UUID)
            .collect();
        assert_eq!(subs.len(), 2, "both subagent files must be found");
        for s in &subs {
            assert_eq!(s.session_id, CLAUDE_UUID, "subagent carries the parent's session id");
            assert_eq!(s.group, main.group, "subagent shares the parent's group");
        }
    }

    #[test]
    fn ids_prefer_the_first_line_and_fall_back_to_the_filename() {
        let r = scanned();
        let good = find(&r, "2026-03-07T14-40-34");
        assert_eq!(good.session_id, CODEX_UUID);
        assert_eq!(good.group, CODEX_UUID);

        // Broken first line -> filename tail uuid, reported rather than swallowed.
        let broken = find(&r, "2026-03-08T10-00-00");
        assert_eq!(broken.session_id, CLAUDE_UUID);
        assert!(
            r.anomalies.iter().any(|a| a.contains("2026-03-08T10-00-00")),
            "the fallback must be reported: {:?}",
            r.anomalies
        );

        let pi = find(&r, "57065825");
        assert_eq!(pi.session_id, PI_UUID, "pi id comes from the session header, not the name");
        assert_eq!(pi.group, PI_UUID);
    }

    #[test]
    fn quiescence_window_marks_recent_files_non_quiescent() {
        let r = scanned();
        assert!(!find(&r, "aaaaaaaa").quiescent, "touched 60s ago: not quiescent");
        assert!(find(&r, "57065825").quiescent, "touched an hour ago: quiescent");
        let summary = summarize(&r);
        assert!(summary.contains("non-quiescent files: 1"), "{summary}");
    }

    #[test]
    fn broken_assumptions_are_reported_and_still_counted() {
        let r = scanned();
        assert!(r.anomalies.iter().any(|a| a.contains("orphaned subagent")), "{:?}", r.anomalies);
        assert!(r.anomalies.iter().any(|a| a.contains("zero-byte")), "{:?}", r.anomalies);
        // The orphaned subagent file is still in the manifest, grouped by its dir.
        assert_eq!(
            r.manifest.entries.iter().filter(|e| e.path.to_string_lossy().contains(CLAUDE_ORPHAN)).count(),
            1
        );
    }

    #[test]
    fn only_jsonl_is_scanned_and_hashes_are_blake3_hex() {
        let r = scanned();
        assert!(r.manifest.entries.iter().all(|e| e.path.extension().unwrap() == "jsonl"));
        for e in &r.manifest.entries {
            assert_eq!(e.blake3.len(), 64);
            assert!(e.blake3.chars().all(|c| c.is_ascii_hexdigit()));
        }
        // BLAKE3's published empty-input vector, so the hash is not just self-consistent.
        assert_eq!(
            find(&r, "broken.jsonl").blake3,
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    #[test]
    fn summarize_reports_per_tool_counts_bytes_subagents_and_quiescence() {
        let s = summarize(&scanned());
        for needle in [
            "pi ",
            "claude ",
            "codex ",
            "bytes",
            "claude groups with subagents: 1",
            "non-quiescent files: 1",
        ] {
            assert!(s.contains(needle), "summary missing {needle:?}:\n{s}");
        }
    }
}
