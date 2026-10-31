//! TRANSPORT — owned by T4. russh + russh-sftp, in-process (decision D5: no
//! exec'd ssh/rsync). Push model (decision D2).
//!
//! Protocol: connect, authenticate by key, open one SFTP channel. The remote
//! blob directory *is* the answer to "what does the archive already hold" —
//! blob names are content hashes, so listing it is the diff, and a blob that
//! is already there is never rewritten. We upload the difference, then the
//! manifest, into the archive host's TAPE_HOME.
//!
//! What this module never does: delete, truncate or overwrite a remote blob.
//! Uploads go to a `<name>.tape.tmp-<pid>` sibling and are renamed into place,
//! so a crashed push leaves an inert temp file (never picked up by `ingest`,
//! which reads `*.tape`) and never a half-written blob under the real name.
//!
//! Host keys are trust-on-first-use, recorded in tape's own
//! `TAPE_HOME/known_hosts` — not `~/.ssh/known_hosts`, because tape must not
//! inherit whatever ssh happened to trust, nor be surprised by ssh rewriting
//! that file underneath it. A *changed* key is a hard error.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use russh::keys::{
    load_secret_key, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate,
};
use russh::{client, Disconnect};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tokio::io::AsyncWriteExt;

use crate::store;
use crate::types::Manifest;

#[derive(Clone, Debug)]
pub struct Remote {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// Private key to authenticate with (unencrypted).
    pub key_path: std::path::PathBuf,
    /// Absolute TAPE_HOME on the archive host, e.g. `/var/lib/tapes`.
    pub tape_home: String,
}

impl Remote {
    fn blobs_dir(&self) -> String {
        format!("{}/store/blobs", self.tape_home.trim_end_matches('/'))
    }
    fn manifests_dir(&self) -> String {
        format!("{}/manifests", self.tape_home.trim_end_matches('/'))
    }
}

#[derive(Debug, Default)]
pub struct PushReport {
    /// Blobs the host already held (content hash matched a name there).
    pub already_present: usize,
    pub uploaded: usize,
    pub uploaded_bytes: u64,
    /// Blobs whose upload was verified by size after rename.
    pub verified: usize,
    /// Manifest entries whose blob is not in the local store.
    pub missing_local: Vec<String>,
    /// Uploads that failed; the run continues past them (it is resumable).
    pub failures: Vec<String>,
    pub manifest_remote: Option<String>,
    pub dry_run: bool,
    /// Blobs the diff said to send (meaningful in a dry run).
    pub to_send: usize,
}

pub fn push(
    manifest: &Manifest,
    remote: &Remote,
    blobs_dir: &Path,
    known_hosts: &Path,
    dry_run: bool,
) -> anyhow::Result<PushReport> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building the tokio runtime for push")?;
    rt.block_on(push_async(
        manifest,
        remote,
        blobs_dir,
        known_hosts,
        dry_run,
    ))
}

/// Everything in the manifest that is eligible to leave this machine:
/// quiescent files only. A file still being written is reported by `scan` and
/// deliberately not archived.
fn wanted_hashes(manifest: &Manifest) -> BTreeSet<String> {
    manifest
        .entries
        .iter()
        .filter(|e| e.quiescent)
        .map(|e| e.blake3.clone())
        .collect()
}

async fn push_async(
    manifest: &Manifest,
    remote: &Remote,
    blobs_dir: &Path,
    known_hosts: &Path,
    dry_run: bool,
) -> anyhow::Result<PushReport> {
    let mut report = PushReport {
        dry_run,
        ..Default::default()
    };

    let wanted = wanted_hashes(manifest);

    // Resolve locally first: no point opening a session if the store is short.
    let mut local: Vec<(String, std::path::PathBuf, u64)> = Vec::new();
    for hash in &wanted {
        let path = store::blob_path(blobs_dir, hash);
        match std::fs::metadata(&path) {
            Ok(meta) => local.push((hash.clone(), path, meta.len())),
            Err(_) => report.missing_local.push(hash.clone()),
        }
    }

    let config = client::Config::default();
    let handler = PushHandler {
        host: remote.host.clone(),
        port: remote.port,
        known_hosts: known_hosts.to_path_buf(),
    };
    let mut session = client::connect(Arc::new(config), (remote.host.as_str(), remote.port), handler)
        .await
        .with_context(|| {
            format!(
                "connecting to {}:{} over ssh",
                remote.host, remote.port
            )
        })?;

    let key = load_secret_key(&remote.key_path, None).with_context(|| {
        format!(
            "loading the private key {} (tape needs an unencrypted key; ssh-agent is not used)",
            remote.key_path.display()
        )
    })?;

    // OpenSSH long ago disabled ssh-rsa (SHA-1); russh maps `None` to it for
    // RSA keys, so ask for sha2 explicitly when the key is RSA.
    let hash_alg = if key.algorithm().is_rsa() {
        Some(HashAlg::Sha256)
    } else {
        None
    };

    let auth = session
        .authenticate_publickey(
            remote.user.clone(),
            PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg),
        )
        .await
        .with_context(|| format!("authenticating as {}", remote.user))?;
    if !auth.success() {
        anyhow::bail!(
            "authentication failed for {}@{}:{} — is this public key in its authorized_keys?",
            remote.user,
            remote.host,
            remote.port
        );
    }

    let channel = session.channel_open_session().await.context("opening a channel")?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .context("requesting the sftp subsystem (is Subsystem sftp enabled?)")?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .context("starting the sftp session")?;

    ensure_dirs(&sftp, &remote.blobs_dir()).await?;
    ensure_dirs(&sftp, &remote.manifests_dir()).await?;

    // The remote directory listing *is* the "what do you already have" reply.
    let have = list_blobs(&sftp, &remote.blobs_dir()).await?;
    for (hash, _, _) in &local {
        if have.contains(hash) {
            report.already_present += 1;
        }
    }
    let to_send: Vec<(String, std::path::PathBuf, u64)> = local
        .into_iter()
        .filter(|(hash, _, _)| !have.contains(hash))
        .collect();
    report.to_send = to_send.len();

    if dry_run {
        tracing::info!(
            "dry run: would upload {} blob(s), {} already present",
            report.to_send,
            report.already_present
        );
        let _ = sftp.close().await;
        return Ok(report);
    }

    let pid = std::process::id();
    for (hash, path, size) in to_send {
        match upload_blob(&sftp, &remote.blobs_dir(), &path, &hash, size, pid).await {
            Ok(()) => {
                report.uploaded += 1;
                report.uploaded_bytes += size;
                report.verified += 1;
            }
            Err(err) => {
                // Resumable by construction: the blob either landed under its
                // hash or it did not. Keep going; report the failure.
                report.failures.push(format!("{hash}: {err:#}"));
            }
        }
    }

    // The manifest travels with the blobs: `ingest` needs it for
    // path/tool/session-id ↔ hash, and `manifest.host` is what lets a second
    // machine share this index. It also carries `only`, so the archive host
    // can tell a subset scan from a whole machine.
    let manifest_remote = format!("{}/{}.json", remote.manifests_dir(), manifest.host);
    let manifest_tmp = format!("{}.tmp-{}", manifest_remote, pid);
    let bytes = serde_json::to_vec_pretty(manifest).context("serializing manifest for push")?;
    write_bytes_remote(&sftp, &manifest_tmp, &bytes).await?;
    sftp.rename(&manifest_tmp, &manifest_remote)
        .await
        .with_context(|| format!("renaming manifest into {}", manifest_remote))?;
    let md = sftp
        .metadata(&manifest_remote)
        .await
        .with_context(|| format!("statting {}", manifest_remote))?;
    anyhow::ensure!(
        md.size == Some(bytes.len() as u64),
        "manifest size mismatch on the remote: {:?} != {}",
        md.size,
        bytes.len()
    );
    report.manifest_remote = Some(manifest_remote);

    let _ = sftp.close().await;
    let _ = session
        .disconnect(Disconnect::ByApplication, "push complete", "en")
        .await;
    Ok(report)
}

struct PushHandler {
    host: String,
    port: u16,
    known_hosts: std::path::PathBuf,
}

impl client::Handler for PushHandler {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // Certificates are not a thing we record; refuse rather than pretend
        // to have checked them.
        if server_public_key.certificate().is_some() {
            tracing::error!(
                "host {} presented a certificate; tape records plain host keys — refusing",
                self.host
            );
            return Ok(false);
        }
        let pk = server_public_key.public_key();
        match russh::keys::check_known_hosts_path(&self.host, self.port, &pk, &self.known_hosts) {
            Ok(true) => Ok(true),
            Ok(false) => {
                let fp = pk.fingerprint(HashAlg::Sha256).to_string();
                tracing::warn!(
                    "unseen host key for {}:{} — {} . Recorded in {}; verify it if you can.",
                    self.host,
                    self.port,
                    fp,
                    self.known_hosts.display()
                );
                record_host_key(&self.known_hosts, &self.host, self.port, &pk)?;
                Ok(true)
            }
            Err(err) => Err(anyhow::anyhow!(
                "host key for {}:{} does NOT match {}: {err}. Refusing to connect. \
                 If this host genuinely has a new key, remove the old line yourself.",
                self.host,
                self.port,
                self.known_hosts.display()
            )),
        }
    }
}

fn record_host_key(
    path: &Path,
    host: &str,
    port: u16,
    pk: &russh::keys::PublicKey,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let token = if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    };
    let line = format!("{token} {}\n", pk.to_openssh()?);
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening known_hosts {}", path.display()))?;
    f.write_all(line.as_bytes())?;
    f.sync_all().ok();
    // A file that silently trusts hosts should not be world-readable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok();
    }
    Ok(())
}

/// mkdir -p over SFTP, which has no recursive create.
async fn ensure_dirs(sftp: &SftpSession, path: &str) -> anyhow::Result<()> {
    let mut acc = String::new();
    for comp in path.trim_start_matches('/').split('/') {
        if comp.is_empty() {
            continue;
        }
        acc.push('/');
        acc.push_str(comp);
        if !sftp
            .try_exists(&acc)
            .await
            .with_context(|| format!("checking {}", acc))?
        {
            sftp.create_dir(&acc)
                .await
                .with_context(|| format!("creating remote dir {}", acc))?;
        }
    }
    Ok(())
}

/// The hashes the archive host already holds. Blob names are `<blake3>.tape`,
/// so the listing is the diff and no server-side helper is needed.
async fn list_blobs(sftp: &SftpSession, dir: &str) -> anyhow::Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    let rd = sftp
        .read_dir(dir)
        .await
        .with_context(|| format!("listing remote {}", dir))?;
    for entry in rd {
        let name = entry.file_name();
        if let Some(hash) = name.strip_suffix(".tape") {
            out.insert(hash.to_string());
        }
    }
    Ok(out)
}

async fn upload_blob(
    sftp: &SftpSession,
    dir: &str,
    local: &Path,
    hash: &str,
    size: u64,
    pid: u32,
) -> anyhow::Result<()> {
    let final_path = format!("{dir}/{}.tape", hash);
    // Not `*.tape`, so `ingest` never sees a partial upload.
    let tmp_path = format!("{dir}/{}.tape.tmp-{}", hash, pid);

    let mut source = std::fs::File::open(local)
        .with_context(|| format!("opening local blob {}", local.display()))?;
    write_stream(sftp, &tmp_path, &mut source, size)
        .await
        .with_context(|| format!("uploading {}", local.display()))?;

    sftp.rename(&tmp_path, &final_path)
        .await
        .with_context(|| format!("renaming into {final_path}"))?;

    let md = sftp
        .metadata(&final_path)
        .await
        .with_context(|| format!("statting {final_path} after upload"))?;
    anyhow::ensure!(
        md.size == Some(size),
        "size mismatch after upload: remote {:?} != local {}",
        md.size,
        size
    );
    Ok(())
}

const CHUNK: usize = 1 << 20;

async fn write_stream(
    sftp: &SftpSession,
    path: &str,
    source: &mut std::fs::File,
    _expect: u64,
) -> anyhow::Result<()> {
    let mut file = sftp
        .open_with_flags(
            path,
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
        )
        .await
        .with_context(|| format!("creating remote {path}"))?;

    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = source
            .read(&mut buf)
            .with_context(|| format!("reading local side of {path}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .await
            .with_context(|| format!("writing {path}"))?;
    }
    file.flush().await.context("flushing")?;
    // Uses the fsync@openssh.com extension when the server offers it, and is a
    // no-op when it does not — never an error.
    file.sync_all().await.ok();
    // close() flushes and releases the server-side handle; until then the
    // rename would race an open file.
    file.close().await.context("closing remote file")?;
    Ok(())
}

/// russh-sftp's own `write` opens with WRITE but not CREATE, so it cannot
/// create the file — which is the one thing push needs.
async fn write_bytes_remote(sftp: &SftpSession, path: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let mut file = sftp
        .open_with_flags(
            path,
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
        )
        .await
        .with_context(|| format!("creating remote {path}"))?;
    file.write_all(bytes)
        .await
        .with_context(|| format!("writing {path}"))?;
    file.flush().await.context("flushing")?;
    file.sync_all().await.ok();
    file.close().await.context("closing remote file")?;
    Ok(())
}

