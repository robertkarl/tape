//! MANIFEST PERSISTENCE — owned by T1.
//!
//! Write/read the scan manifest, and keep the host-side "already seen" record
//! so a re-scan only re-hashes what changed.

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
}
