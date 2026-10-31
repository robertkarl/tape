//! INDEX — owned by T3. rusqlite with `bundled-sqlcipher` (decision D4).
//! Schema is specified verbatim in PLAN.md §4 — use that DDL, do not invent
//! a second one.

use std::path::Path;

pub fn open(path: &Path, key: &str) -> anyhow::Result<rusqlite::Connection> {
    let _ = (path, key);
    anyhow::bail!("not implemented (T3): open/create the SQLCipher index and apply migrations")
}
