//! MANIFEST PERSISTENCE — owned by T1.
//!
//! Write/read the scan manifest, and keep the host-side "already seen" record
//! so a re-scan only re-hashes what changed.

use std::path::Path;

use crate::types::Manifest;

pub fn write_manifest(m: &Manifest, at: &Path) -> anyhow::Result<()> {
    let _ = m;
    let _ = at;
    anyhow::bail!("not implemented (T1): write manifest atomically (write -> fsync -> rename)")
}

pub fn read_manifest(at: &Path) -> anyhow::Result<Manifest> {
    let _ = at;
    anyhow::bail!("not implemented (T1): read manifest")
}
