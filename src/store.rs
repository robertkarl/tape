//! BLOB STORE — owned by T2.
//!
//! zstd + age, content-addressed, immutable. Invariants (non-negotiable):
//!   * a blob is written as write -> fsync -> rename; a killed process must
//!     never leave an unreadable blob at its final name;
//!   * the blob's identity is blake3 of the ORIGINAL plaintext, so a superseded
//!     version of a grown session is simply a different blob;
//!   * nothing here ever deletes or rewrites an existing blob — not even temp
//!     files, which are reported and left for a human.
//!
//! Blob format on disk: `age_encrypt(zstd(raw_file_bytes))`. The plaintext
//! recovered by `verify` is byte-identical to the source transcript; no
//! normalization ever touches Layer 1.

use std::fs::{self, File, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use age::secrecy::ExposeSecret;
use age::x25519;
use chrono::Utc;

use crate::scan::QUIESCENCE_MINUTES;
use crate::types::{BlobRef, FileEntry, Tool};

/// zstd level used for every blob.
pub const ZSTD_LEVEL: i32 = 12;

/// Default parallelism for store/verify (zstd at level 12 is the bottleneck).
pub fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(16)
}

#[derive(Debug, Default)]
pub struct StoreReport {
    pub stored: usize,
    pub already_present: usize,
    pub skipped_not_quiescent: usize,
    pub failures: Vec<String>,
    pub plaintext_bytes: u64,
    pub ciphertext_bytes: u64,
}

impl StoreReport {
    pub fn ratio(&self) -> f64 {
        if self.ciphertext_bytes == 0 {
            0.0
        } else {
            self.plaintext_bytes as f64 / self.ciphertext_bytes as f64
        }
    }
}

#[derive(Debug, Default)]
pub struct VerifyReport {
    pub checked: usize,
    pub ok: usize,
    /// (blob path, reason)
    pub failures: Vec<(PathBuf, String)>,
    pub plaintext_bytes: u64,
    pub ciphertext_bytes: u64,
}

// ---------------------------------------------------------------------------
// keys
// ---------------------------------------------------------------------------

/// Generate an age X25519 keypair. Refuses to overwrite either output: losing
/// the identity makes every stored blob permanently unreadable.
pub fn keygen(out_identity: &Path, out_pubkey: &Path) -> anyhow::Result<()> {
    for p in [out_identity, out_pubkey] {
        if p.exists() {
            anyhow::bail!(
                "refusing to overwrite existing key material at {} \
                 (a lost identity makes every blob unreadable)",
                p.display()
            );
        }
    }
    if let Some(dir) = out_identity.parent() {
        fs::create_dir_all(dir)?;
    }

    let identity = x25519::Identity::generate();
    let pubkey = identity.to_public();
    let secret = identity.to_string();

    // create_new + 0400 before any byte is written: the key is never readable
    // by anyone but the owner, not even briefly.
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out_identity)?;
    f.set_permissions(Permissions::from_mode(0o400))?;
    writeln!(
        f,
        "# tape age identity — DO NOT DELETE, DO NOT COMMIT, BACK IT UP."
    )?;
    writeln!(
        f,
        "# Losing this file makes every stored blob permanently unreadable."
    )?;
    writeln!(f, "{}", secret.expose_secret())?;
    f.flush()?;
    f.sync_all()?;
    drop(secret);

    if let Some(dir) = out_pubkey.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out_pubkey)?;
    writeln!(
        f,
        "# tape archive public key — safe to copy onto source hosts"
    )?;
    writeln!(f, "{pubkey}")?;
    f.flush()?;
    f.sync_all()?;

    println!("identity: {}", out_identity.display());
    println!("pubkey:   {pubkey}");
    Ok(())
}

/// Read tape's identity file (comments allowed, first parseable line wins).
pub fn load_identity(path: &Path) -> anyhow::Result<x25519::Identity> {
    let raw = fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read identity {}: {e}", path.display()))?;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.parse::<x25519::Identity>() {
            Ok(id) => return Ok(id),
            Err(e) => anyhow::bail!("{}: not an age X25519 identity: {e:?}", path.display()),
        }
    }
    anyhow::bail!("{} contains no age identity line", path.display())
}

pub fn load_recipient(path: &Path) -> anyhow::Result<x25519::Recipient> {
    let raw = fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read pubkey {}: {e}", path.display()))?;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.parse::<x25519::Recipient>() {
            Ok(r) => return Ok(r),
            Err(e) => anyhow::bail!("{}: not an age public key: {e:?}", path.display()),
        }
    }
    anyhow::bail!("{} contains no age public key", path.display())
}

// ---------------------------------------------------------------------------
// blob primitives
// ---------------------------------------------------------------------------

pub fn blob_name(hash: &str) -> String {
    format!("{hash}.tape")
}

pub fn blob_path(blobs_dir: &Path, hash: &str) -> PathBuf {
    blobs_dir.join(blob_name(hash))
}

/// write -> fsync -> rename. The final name only ever appears once the whole
/// ciphertext is on disk. Temp files are left behind, never deleted.
pub fn atomic_write(final_path: &Path, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let tmp = final_path.with_extension(format!("tape.tmp-{}", std::process::id()));
    {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, final_path)?;
    // Directory fsync so the rename itself survives a crash. macOS returns
    // EINVAL for this; the rename is still atomic either way.
    if let Some(dir) = final_path.parent() {
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(tmp)
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn is_quiescent(mtime_ms: i64, now_ms: i64) -> bool {
    now_ms - mtime_ms >= (QUIESCENCE_MINUTES as i64) * 60_000
}

/// zstd -> age -> bytes.
pub fn seal(plaintext: &[u8], recipient: &x25519::Recipient) -> anyhow::Result<Vec<u8>> {
    let compressed = zstd::encode_all(plaintext, ZSTD_LEVEL)?;
    let ciphertext = age::encrypt(recipient, &compressed)?;
    Ok(ciphertext)
}

/// age -> zstd -> bytes, asserting the result hashes to `expect_hash`.
pub fn open_and_check(
    ciphertext: &[u8],
    identity: &x25519::Identity,
    expect_hash: &str,
) -> anyhow::Result<Vec<u8>> {
    let compressed = age::decrypt(identity, ciphertext)?;
    let plaintext = zstd::decode_all(&compressed[..])?;
    let got = blake3::hash(&plaintext).to_hex().to_string();
    if got != expect_hash {
        anyhow::bail!("hash mismatch: blob says {expect_hash}, contents hash to {got}");
    }
    Ok(plaintext)
}

/// Store one file's bytes under its content hash. Returns (blob, newly_written).
pub fn store_bytes(
    plaintext: &[u8],
    recipient: &x25519::Recipient,
    blobs_dir: &Path,
) -> anyhow::Result<(BlobRef, bool)> {
    let hash = blake3::hash(plaintext).to_hex().to_string();
    let dest = blob_path(blobs_dir, &hash);
    if dest.exists() {
        let ct = fs::metadata(&dest)?.len();
        return Ok((
            BlobRef {
                hash,
                path: dest,
                ciphertext_bytes: ct,
                plaintext_bytes: plaintext.len() as u64,
                stored_at: Utc::now(),
            },
            false,
        ));
    }
    let ciphertext = seal(plaintext, recipient)?;
    // create_dir_all is idempotent and safe to call concurrently.
    if !blobs_dir.exists() {
        fs::create_dir_all(blobs_dir)?;
    }
    atomic_write(&dest, &ciphertext)?;
    Ok((
        BlobRef {
            hash,
            path: dest,
            ciphertext_bytes: ciphertext.len() as u64,
            plaintext_bytes: plaintext.len() as u64,
            stored_at: Utc::now(),
        },
        true,
    ))
}

pub fn store_blob(entry: &FileEntry, pubkey: &Path, blobs_dir: &Path) -> anyhow::Result<BlobRef> {
    let recipient = load_recipient(pubkey)?;
    let plaintext = fs::read(&entry.path)?;
    if entry.quiescent {
        let actual = blake3::hash(&plaintext).to_hex().to_string();
        if actual != entry.blake3 {
            anyhow::bail!(
                "{} changed between scan and store (manifest says {}, file hashes to {})",
                entry.path.display(),
                entry.blake3,
                actual
            );
        }
    }
    let (blob, _new) = store_bytes(&plaintext, &recipient, blobs_dir)?;
    Ok(blob)
}

// ---------------------------------------------------------------------------
// bulk operations
// ---------------------------------------------------------------------------

pub fn store_all(
    entries: &[FileEntry],
    pubkey_path: &Path,
    blobs_dir: &Path,
    jobs: usize,
) -> anyhow::Result<StoreReport> {
    let recipient = load_recipient(pubkey_path)?;
    fs::create_dir_all(blobs_dir)?;

    let jobs = jobs.max(1).min(entries.len().max(1));
    let stored = AtomicUsize::new(0);
    let present = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(0);
    let pt_bytes = AtomicU64::new(0);
    let ct_bytes = AtomicU64::new(0);
    let failures: Mutex<Vec<String>> = Mutex::new(Vec::new());

    // References are Copy: each spawned thread gets its own copy of the
    // pointers to the shared counters.
    let ctx = (
        &stored, &present, &skipped, &pt_bytes, &ct_bytes, &failures, &recipient, blobs_dir,
    );
    let chunk = entries.len().div_ceil(jobs).max(1);
    std::thread::scope(|scope| {
        for group in entries.chunks(chunk) {
            let (stored, present, skipped, pt_bytes, ct_bytes, failures, recipient, blobs_dir) =
                ctx;
            scope.spawn(move || {
                for e in group {
                    if !e.quiescent {
                        skipped.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    match (|| -> anyhow::Result<(bool, u64, u64)> {
                        let plaintext = fs::read(&e.path)?;
                        let actual = blake3::hash(&plaintext).to_hex().to_string();
                        if actual != e.blake3 {
                            anyhow::bail!(
                                "changed since scan: manifest {} vs file {}",
                                e.blake3,
                                actual
                            );
                        }
                        let (blob, new) = store_bytes(&plaintext, recipient, blobs_dir)?;
                        Ok((new, blob.plaintext_bytes, blob.ciphertext_bytes))
                    })() {
                        Ok((new, pt, ct)) => {
                            pt_bytes.fetch_add(pt, Ordering::Relaxed);
                            ct_bytes.fetch_add(ct, Ordering::Relaxed);
                            if new {
                                stored.fetch_add(1, Ordering::Relaxed);
                            } else {
                                present.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        Err(err) => {
                            failures
                                .lock()
                                .unwrap()
                                .push(format!("{}: {err}", e.path.display()));
                        }
                    }
                }
            });
        }
    });

    Ok(StoreReport {
        stored: stored.load(Ordering::Relaxed),
        already_present: present.load(Ordering::Relaxed),
        skipped_not_quiescent: skipped.load(Ordering::Relaxed),
        failures: failures.into_inner().unwrap_or_default(),
        plaintext_bytes: pt_bytes.load(Ordering::Relaxed),
        ciphertext_bytes: ct_bytes.load(Ordering::Relaxed),
    })
}

/// One blob: decrypt, decompress, re-hash against its filename.
pub fn verify_blob(blob: &Path, identity_path: &Path) -> anyhow::Result<u64> {
    let identity = load_identity(identity_path)?;
    verify_blob_with(blob, &identity)
}

pub fn verify_blob_with(blob: &Path, identity: &x25519::Identity) -> anyhow::Result<u64> {
    let stem = blob
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("blob has no filename stem"))?
        .to_string();
    if stem.len() != 64 {
        anyhow::bail!("blob name is not a blake3 hex digest: {stem}");
    }
    let ciphertext = fs::read(blob)?;
    let plaintext = open_and_check(&ciphertext, identity, &stem)?;
    Ok(plaintext.len() as u64)
}

pub fn verify_all(
    blobs_dir: &Path,
    identity_path: &Path,
    jobs: usize,
) -> anyhow::Result<VerifyReport> {
    if !blobs_dir.is_dir() {
        anyhow::bail!("no blob store at {}", blobs_dir.display());
    }
    let identity = load_identity(identity_path)?;
    let mut blobs: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(blobs_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("tape") {
            blobs.push(path);
        } else if path
            .file_name()
            .map(|n| n.to_string_lossy().contains(".tape.tmp-"))
            .unwrap_or(false)
        {
            // Orphans from a killed run. Reported, never removed.
            tracing::warn!("orphaned temp blob left behind: {}", path.display());
        }
    }
    blobs.sort();

    let jobs = jobs.max(1).min(blobs.len().max(1));
    let checked = AtomicUsize::new(0);
    let ok = AtomicUsize::new(0);
    let pt_bytes = AtomicU64::new(0);
    let ct_bytes = AtomicU64::new(0);
    let failures: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());

    let ctx = (&checked, &ok, &pt_bytes, &ct_bytes, &failures, &identity);
    let chunk = blobs.len().div_ceil(jobs).max(1);
    std::thread::scope(|scope| {
        for group in blobs.chunks(chunk) {
            let (checked, ok, pt_bytes, ct_bytes, failures, identity) = ctx;
            scope.spawn(move || {
                for blob in group {
                    checked.fetch_add(1, Ordering::Relaxed);
                    match (|| -> anyhow::Result<(u64, u64)> {
                        let ct = fs::metadata(blob)?.len();
                        let pt = verify_blob_with(blob, identity)?;
                        Ok((pt, ct))
                    })() {
                        Ok((pt, ct)) => {
                            ok.fetch_add(1, Ordering::Relaxed);
                            pt_bytes.fetch_add(pt, Ordering::Relaxed);
                            ct_bytes.fetch_add(ct, Ordering::Relaxed);
                        }
                        Err(err) => failures
                            .lock()
                            .unwrap()
                            .push((blob.clone(), format!("{err:#}"))),
                    }
                }
            });
        }
    });

    Ok(VerifyReport {
        checked: checked.load(Ordering::Relaxed),
        ok: ok.load(Ordering::Relaxed),
        failures: failures.into_inner().unwrap_or_default(),
        plaintext_bytes: pt_bytes.load(Ordering::Relaxed),
        ciphertext_bytes: ct_bytes.load(Ordering::Relaxed),
    })
}

// ---------------------------------------------------------------------------
// STOPGAP walker
// ---------------------------------------------------------------------------
//
// T1 owns the real producer (`scan::scan`). This exists only so the crypto
// layer can be proven end-to-end before that lands: it walks a tree, hashes
// every .jsonl, and fills FileEntry with a path-shape guess for `tool`.
// Delete it once `tape store` reads a manifest.

pub fn walk_for_verification(root: &Path) -> anyhow::Result<Vec<FileEntry>> {
    let mut out = Vec::new();
    let now = now_ms();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let meta = entry.metadata()?;
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let bytes = fs::read(path)?;
        let s = path.to_string_lossy();
        let tool = if s.contains("/.claude/") {
            Tool::Claude
        } else if s.contains("/.codex/") {
            Tool::Codex
        } else if s.contains("/.pi/") {
            Tool::Pi
        } else {
            continue;
        };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        out.push(FileEntry {
            tool,
            session_id: stem.clone(),
            group: stem,
            path: path.to_path_buf(),
            size: meta.len(),
            mtime_ms,
            blake3: blake3::hash(&bytes).to_hex().to_string(),
            quiescent: is_quiescent(mtime_ms, now),
        });
    }
    Ok(out)
}
