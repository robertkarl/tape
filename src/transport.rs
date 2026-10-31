//! TRANSPORT — owned by T4. russh + russh-sftp, in-process (decision D5: no
//! exec'd ssh/rsync). Push model (decision D2).
//!
//! Protocol: send our manifest, the archive host replies with the set of blob
//! hashes it already holds, we upload only the difference. A blob already
//! present on the host is never overwritten.

use std::path::Path;

use crate::types::Manifest;

pub struct Remote {
    pub host: String,
    pub port: u16,
    pub user: String,
}

#[derive(Debug, Default)]
pub struct PushReport {
    pub already_present: usize,
    pub uploaded: usize,
    pub uploaded_bytes: u64,
}

pub fn push(manifest: &Manifest, remote: &Remote, store: &Path) -> anyhow::Result<PushReport> {
    let _ = (manifest, remote, store);
    anyhow::bail!("not implemented (T4): sftp push of missing blobs")
}
