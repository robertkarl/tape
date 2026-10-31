// Stub signatures across modules are intentional while work units land in
// parallel worktrees; silence the resulting noise rather than delete the
// contracts.
#![allow(dead_code)]

mod db;
mod manifest;
mod parse;
mod paths;
mod render;
mod scan;
mod store;
mod transport;
mod types;
mod web;

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
    /// zstd+age new blobs into the content-addressed store
    Store,
    /// Decrypt + re-hash every stored blob and report failures
    Verify,
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
            let m = scan::scan()?;
            if json {
                println!("{}", serde_json::to_string(&m)?);
            } else {
                println!("{} entries", m.entries.len());
            }
            Ok(())
        }
        Cmd::Keygen => store::keygen(&paths::identity_path(), &paths::pubkey_path()),
        Cmd::Store => anyhow::bail!("not implemented (T2)"),
        Cmd::Verify => anyhow::bail!("not implemented (T2)"),
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
