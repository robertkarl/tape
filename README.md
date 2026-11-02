# tape

A local-first, **append-only archive of agent transcripts** — the JSONL that
pi, Claude Code and Codex write about the work they did — copied off the
machine that produces them onto a box that keeps them forever.

The tools prune their own history. This archive does not: blobs are
content-addressed, encrypted, and never rewritten or deleted, so a session that
a tool later removed is still here, and `tape audit` can tell you when it
vanished.

```
tape scan      what the tools hold right now (reads only their trees)
tape store     zstd + age -> store/blobs/<blake3>.tape
tape verify    decrypt, re-hash, report
tape ingest    decrypt blobs -> SQLCipher index (sessions / messages / tool_calls)
tape stats     per-tool counts, or one read-only query
tape push      send missing blobs to the archive host over SFTP
tape serve     browse UI on the LAN                               (live on the archive host)
tape audit     diff source vs archive, record vanished sessions   (not yet)
```

## Tenets

- **Local first.** No outbound calls from the archive host after setup. No cloud.
- **Append-only.** Nothing in this codebase deletes or rewrites a blob.
- **Encrypted at rest and in transit.** Blobs: `age` X25519 over zstd-12.
  Index: SQLCipher. Transport: SSH. The web UI is LAN-only plaintext HTTP by
  explicit decision, not by accident.
- **Compiled Rust only.** One binary, one database, no daemon besides `serve`.
- **The index is disposable.** Layer 1 (blobs) is the archive; Layer 2 (SQLite)
  is a projection that `tape ingest` can rebuild from scratch.

## Layout

| Path | What |
|---|---|
| `SPEC.md` | the requirements, in the original words |
| `PLAN.md` | architecture, storage, encryption, cross-tool normalization, milestones |
| `TASKS.md` | the six work units, ownership boundaries, acceptance criteria |
| `STATE.md` | current snapshot: what is built, measured numbers, open items |
| `prompts/` | self-contained briefs handed to implementing agents |

## Status

Built and measured on real transcripts: `scan`, `keygen`, `store`, `verify`,
`ingest`, `stats`. 846 blobs from ~2.4 GB of JSONL, 2.69x smaller, 740 sessions,
93k messages, 55k tool calls, zero parse failures.

Built and measured end to end: `push` — one SSH connection, one SFTP channel,
the remote blob listing *is* the diff, uploads renamed into place, host keys
trust-on-first-use in tape's own `known_hosts`. The archive host is a Proxmox
container on a spinning RAIDZ2 pool; its private age identity never leaves it,
so source machines hold only the public key.

Built: `serve` — axum, LAN bind only, no auth by decision. The list page reads
the index; a session page decrypts that session's blobs, re-verifies each
content hash, parses, and renders one self-contained HTML page (timeline rail,
density modes, one turn at a time, edits as diffs). Read-only at the
connection level: `PRAGMA query_only` before the first query.

Built: the launchd job (`deploy/net.robertkarl.tapes.plist`, every 15 min) and
the CT-side `tape-ingest` / `tape-verify` systemd timers — see `deploy/README.md`.

Not built: `audit`.

Scope: the whole machine. `tape scan` with no `--only` walks all four roots; the
earlier one-project subset (decision D9) was lifted on 2026-11-02 and must stay
lifted, because the archive host takes the **newest manifest per host** and a
subset push would make everything outside it look vanished to `audit`.

## Build

```bash
cargo build          # deps: russh, axum, rusqlite/bundled-sqlcipher, age, zstd
cargo test           # 70 tests
```

`age` is pinned to 0.11 deliberately — see the comment in `Cargo.toml`.

## License

MIT — see `LICENSE`. One exception worth knowing: `src/render/template.rs` is a
translation of `render_trace.py` from a separate private project of the same
author, and its header attribution is kept in the file.
