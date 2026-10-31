//! MANIFEST PERSISTENCE — owned by T1.
//!
//! Write/read the scan manifest, and keep the host-side "already seen" record
//! so a re-scan only re-hashes what changed.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::Context;

use crate::types::Manifest;

/// Atomic write: serialize to a sibling temp file, fsync the file, rename it
/// over the target, fsync the parent directory. A crashed run therefore never
/// leaves a half-written manifest at `at`.
pub fn write_manifest(m: &Manifest, at: &Path) -> anyhow::Result<()> {
    let parent = at
        .parent()
        .with_context(|| format!("manifest path has no parent: {}", at.display()))?;
    std::fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;

    let bytes = serde_json::to_vec_pretty(m).context("serialize manifest")?;

    let tmp = at.with_extension(format!("json.tmp.{}", std::process::id()));
    {
        let mut file = std::fs::File::create(&tmp)
            .with_context(|| format!("create temp manifest {}", tmp.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("write temp manifest {}", tmp.display()))?;
        file.flush().context("flush temp manifest")?;
        file.sync_all().context("fsync temp manifest")?;
    }
    std::fs::rename(&tmp, at)
        .with_context(|| format!("rename {} -> {}", tmp.display(), at.display()))?;

    // Durability of the rename itself (best effort on some platforms).
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

pub fn read_manifest(at: &Path) -> anyhow::Result<Manifest> {
    let bytes = std::fs::read(at).with_context(|| format!("read manifest {}", at.display()))?;
    let m: Manifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse manifest {}", at.display()))?;
    Ok(m)
}

/// Every manifest tape knows about: this host's own `manifest.json` plus the
/// per-host manifests that `tape push` delivered into `TAPE_HOME/manifests/`.
///
/// On the source machine this is normally one file. On the archive host it is
/// one per source host, which is what keeps a second machine's archive in the
/// same index without colliding (`blobs.host` / `sessions.host` come from the
/// manifest, not from the machine running `ingest`).
///
/// For a given host, only the newest `generated_at` is kept: an older manifest
/// for the same host adds nothing that the newer one does not already cover,
/// and merging both would double-count entries whose hash has since moved on.
pub fn load_all() -> anyhow::Result<Vec<Manifest>> {
    load_all_from(&crate::paths::manifest_path(), &crate::paths::manifests_dir())
}

/// The testable core of `load_all`: both locations are injected.
pub fn load_all_from(own: &Path, dir: &Path) -> anyhow::Result<Vec<Manifest>> {
    let mut by_host: std::collections::BTreeMap<String, Manifest> = Default::default();

    if own.exists() {
        let m = read_manifest(own)?;
        by_host
            .entry(m.host.clone())
            .and_modify(|cur| {
                if m.generated_at > cur.generated_at {
                    *cur = m.clone();
                }
            })
            .or_insert(m);
    }

    if dir.is_dir() {
        let mut files: Vec<_> = fs::read_dir(&dir)
            .with_context(|| format!("read dir {}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
            .collect();
        files.sort();
        for f in files {
            let m = read_manifest(&f)?;
            by_host
                .entry(m.host.clone())
                .and_modify(|cur| {
                    if m.generated_at > cur.generated_at {
                        *cur = m.clone();
                    }
                })
                .or_insert(m);
        }
    }

    Ok(by_host.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FileEntry, Manifest, Tool};
    use chrono::{TimeZone, Utc};
    use std::fs;

    fn sample_manifest() -> Manifest {
        Manifest {
            host: "testhost".into(),
            generated_at: Utc.timestamp_millis_opt(1_793_483_981_000).unwrap(),
            only: None,
            entries: vec![FileEntry {
                tool: Tool::Claude,
                session_id: "6f1d2e10-1111-4aaa-8bbb-0000000000c1".into(),
                group: "6f1d2e10-1111-4aaa-8bbb-0000000000c1".into(),
                path: "/tmp/does-not-matter/6f1d2e10-1111-4aaa-8bbb-0000000000c1.jsonl".into(),
                size: 12,
                mtime_ms: 1_793_480_000_000,
                blake3: "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262".into(),
                quiescent: true,
            }],
        }
    }

    #[test]
    fn manifest_round_trips_and_creates_its_parent_dir() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        // Scratch stays in /tmp; the house rule is no `rm`, ever.
        let at = Path::new(&format!(
            "/tmp/tape-t1-manifest-{}-{nanos}",
            std::process::id()
        ))
        .join("nested/manifest.json");

        let m = sample_manifest();
        write_manifest(&m, &at).unwrap();
        let back = read_manifest(&at).unwrap();

        assert_eq!(back.host, m.host);
        assert_eq!(back.entries.len(), 1);
        let e = &back.entries[0];
        assert_eq!(e.blake3, m.entries[0].blake3);
        assert_eq!(e.group, m.entries[0].group);
        assert!(e.quiescent);
        assert_eq!(back.generated_at, m.generated_at);

        // The atomic rename leaves no temp file beside the manifest.
        let leftovers: Vec<_> = fs::read_dir(at.parent().unwrap())
            .unwrap()
            .filter_map(|de| de.ok())
            .filter(|de| {
                de.path()
                    .file_name()
                    .map(|n| n.to_string_lossy().contains("tmp"))
                    .unwrap_or(false)
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "atomic write left temp debris: {leftovers:?}"
        );
    }

    #[test]
    fn a_pre_d9_manifest_without_the_only_field_still_parses() {
        // D9 added `only`. Blobs and manifests written before it must stay
        // readable — the archive is append-only, its metadata has to be too.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let at = Path::new(&format!("/tmp/tape-manifest-oldfmt-{nanos}")).join("manifest.json");
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        fs::write(
            &at,
            r#"{"host":"oldhost","generated_at":"2026-10-31T00:00:00Z","entries":[]}"#,
        )
        .unwrap();
        let m = read_manifest(&at).unwrap();
        assert_eq!(m.host, "oldhost");
        assert_eq!(m.only, None);
    }

    #[test]
    fn load_all_keeps_one_manifest_per_host_and_the_newest_wins() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir_str = format!("/tmp/tape-manifest-loadall-{nanos}");
        let root = Path::new(&dir_str);
        let own = root.join("manifest.json");
        let dir = root.join("manifests");

        let mut mine = sample_manifest(); // host "testhost", generated 1793483981000
        mine.only = Some("/home/me/Code/project".into());
        write_manifest(&mine, &own).unwrap();

        // A newer manifest for the SAME host (a later scan of this machine,
        // pushed here) must supersede the local one, not add to it.
        let mut newer = sample_manifest();
        newer.generated_at = mine.generated_at + chrono::Duration::seconds(60);
        write_manifest(&newer, &dir.join("testhost.json")).unwrap();

        // A different host is a different archive, kept side by side.
        let mut other = sample_manifest();
        other.host = "otherhost".into();
        write_manifest(&other, &dir.join("otherhost.json")).unwrap();

        let all = load_all_from(&own, &dir).unwrap();
        assert_eq!(all.len(), 2, "one per host: {all:?}");
        let mine_back = all.iter().find(|m| m.host == "testhost").unwrap();
        assert_eq!(
            mine_back.generated_at, newer.generated_at,
            "newest manifest for a host wins"
        );
        assert!(all.iter().any(|m| m.host == "otherhost"));
    }
}
