// Stub signatures across modules are intentional while work units land in
// parallel worktrees; silence the resulting noise rather than delete the
// contracts.
#![allow(dead_code)]

use tape::{manifest, paths, scan, store, transport, web};

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "tape",
    about = "Local-first, append-only transcript archive for pi / Claude Code / Codex",
    version
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List transcripts the tools currently hold (writes nothing outside TAPE_HOME)
    Scan {
        /// Emit the manifest as JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Generate the age keypair (refuses to overwrite an existing identity)
    Keygen,
    /// zstd+age new blobs into the content-addressed store (reads the
    /// manifest written by `tape scan`)
    Store {
        /// Parallel zstd/age jobs
        #[arg(long, default_value_t = store::default_jobs())]
        jobs: usize,
    },
    /// Decrypt + re-hash every stored blob and report failures
    Verify {
        #[arg(long)]
        blobs: Option<PathBuf>,
        #[arg(long, default_value_t = store::default_jobs())]
        jobs: usize,
    },
    /// Push missing blobs to the archive host over sftp
    Push {
        host: String,
        #[arg(long, default_value_t = 22)]
        port: u16,
        #[arg(long, default_value = "rk")]
        user: String,
    },
    /// Archive-host side: ingest received blobs into the index
    Ingest {
        #[arg(long)]
        store: PathBuf,
    },
    /// Serve the browse UI (LAN bind only)
    Serve {
        #[arg(long, default_value = "0.0.0.0")]
        addr: String,
        #[arg(long, default_value_t = 8799)]
        port: u16,
    },
    /// Diff what the tools hold now against what the archive holds; record
    /// vanished session ids (the deletion-detection instrument)
    Audit,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    match Cli::parse().cmd {
        Cmd::Scan { json } => {
            let result = scan::scan_result()?;
            if json {
                println!("{}", serde_json::to_string(&result.manifest)?);
            } else {
                println!("{}", scan::summarize(&result));
            }
            Ok(())
        }
        Cmd::Keygen => store::keygen(&paths::identity_path(), &paths::pubkey_path()),
        Cmd::Store { jobs } => {
            let m = paths::manifest_path();
            if !m.exists() {
                anyhow::bail!("no manifest at {}; run `tape scan` first", m.display());
            }
            let entries = manifest::read_manifest(&m)?.entries;
            println!("{} candidate files", entries.len());
            let report =
                store::store_all(&entries, &paths::pubkey_path(), &paths::blobs_dir(), jobs)?;
            println!("stored:            {}", report.stored);
            println!("already present:   {}", report.already_present);
            println!("not quiescent:     {}", report.skipped_not_quiescent);
            println!("failures:          {}", report.failures.len());
            for f in report.failures.iter().take(20) {
                println!("  {f}");
            }
            println!(
                "plaintext:         {} MiB",
                report.plaintext_bytes / 1_048_576
            );
            println!(
                "ciphertext:        {} MiB ({}x)",
                report.ciphertext_bytes / 1_048_576,
                format!("{:.2}", report.ratio())
            );
            if !report.failures.is_empty() {
                anyhow::bail!("{} blob writes failed", report.failures.len());
            }
            Ok(())
        }
        Cmd::Verify { blobs, jobs } => {
            let dir = blobs.unwrap_or_else(paths::blobs_dir);
            let report = store::verify_all(&dir, &paths::identity_path(), jobs)?;
            println!("checked:    {}", report.checked);
            println!("ok:         {}", report.ok);
            println!("failures:   {}", report.failures.len());
            for (path, why) in report.failures.iter().take(20) {
                println!("  {}: {why}", path.display());
            }
            println!("plaintext:  {} MiB", report.plaintext_bytes / 1_048_576);
            println!("ciphertext: {} MiB", report.ciphertext_bytes / 1_048_576);
            if report.checked != report.ok {
                anyhow::bail!(
                    "verify failed: {} of {} blobs bad",
                    report.checked - report.ok,
                    report.checked
                );
            }
            Ok(())
        }
        Cmd::Push { host, port, user } => {
            let m = scan::scan()?;
            let r = transport::push(
                &m,
                &transport::Remote { host, port, user },
                &paths::blobs_dir(),
            )?;
            println!("{r:?}");
            Ok(())
        }
        Cmd::Ingest { store } => {
            let _ = store;
            anyhow::bail!("not implemented (T4)")
        }
        Cmd::Serve { addr, port } => web::serve(&addr, port),
        Cmd::Audit => anyhow::bail!("not implemented (T6)"),
    }
}
