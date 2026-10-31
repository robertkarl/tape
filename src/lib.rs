//! tape — local-first, append-only transcript archive.
//!
//! Library target so the modules can be unit-tested from `tests/`; the binary
//! in `src/main.rs` is a thin clap front-end over these modules.
//! Stub signatures across modules are intentional while work units land in
//! parallel worktrees.
#![allow(dead_code)]

pub mod db;
pub mod ingest;
pub mod manifest;
pub mod parse;
pub mod paths;
pub mod render;
pub mod scan;
pub mod store;
pub mod transport;
pub mod types;
pub mod web;
