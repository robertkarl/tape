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
tape push      send missing blobs to the archive host over SFTP   (not yet)
tape serve     browse UI on the LAN                               (not yet)
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

Not built: `push`, `serve`, `audit`, and the launchd job that makes the copy
automatic. See `STATE.md`.

Working scope is deliberately narrow: until the pipeline is proven end to end,
tape archives **one project's sessions** — whatever `tape scan --only <path>`
selects — rather than the whole machine. See PLAN §2.1 (decision D9).

## Build

```bash
cargo build          # deps: russh, axum, rusqlite/bundled-sqlcipher, age, zstd
cargo test           # 37 tests
```

`age` is pinned to 0.11 deliberately — see the comment in `Cargo.toml`.
