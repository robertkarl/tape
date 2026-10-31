//! BLOB STORE — owned by T2.
//!
//! zstd + age, content-addressed, immutable. Invariants (non-negotiable):
//!   * a blob is written as write -> fsync -> rename; a killed process must
//!     never leave an unreadable blob at its final name;
//!   * the blob's identity is blake3 of the ORIGINAL plaintext, so a superseded
//!     version of a grown session is simply a different blob;
//!   * nothing here ever deletes or rewrites an existing blob.

use std::path::Path;

use crate::types::{BlobRef, FileEntry};

/// zstd level used for every blob.
pub const ZSTD_LEVEL: i32 = 12;

pub fn keygen(out_identity: &Path, out_pubkey: &Path) -> anyhow::Result<()> {
    let _ = (out_identity, out_pubkey);
    anyhow::bail!("not implemented (T2): age X25519 keypair; refuse to overwrite an existing identity")
}

pub fn store_blob(entry: &FileEntry, identity: &Path, blobs: &Path) -> anyhow::Result<BlobRef> {
    let _ = (entry, identity, blobs);
    anyhow::bail!("not implemented (T2): zstd+age a file into the content-addressed store")
}

/// Decrypt, decompress, re-hash, compare against the blob's name.
pub fn verify_blob(blob: &Path, identity: &Path) -> anyhow::Result<()> {
    let _ = (blob, identity);
    anyhow::bail!("not implemented (T2): round-trip one blob and check its hash")
}
