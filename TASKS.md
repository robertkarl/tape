# TASKS.md — handoff brief for agents

Read `SPEC.md` then `PLAN.md` first. This file turns PLAN.md §11 (answered) into
work units. Decisions are **locked** — do not re-litigate them in code.

## Locked decisions (from PLAN.md §11, 2026-10-31)

| # | Decision | Answer |
|---|---|---|
| D1 | Build the Linux binary | **Inside CT 127** (internet at build time, before the egress cut) |
| D2 | Scheduler location | **Source pushes** (debra launchd → CT) |
| D3 | Web UI exposure | **LAN-plaintext HTTP**, LAN bind only. No nginx, no certs, no DNS |
| D4 | Index at rest | **SQLCipher** (`rusqlite` `bundled-sqlcipher`) |
| D5 | Transport | **`russh` + `russh-sftp`**, in-process, no exec'd `ssh` |
| D6 | Name | **`tape`** |
| D7 | Storage | CT 127 on proxmox; data on the **spinning RAIDZ2 pool `tank`** → host path `/tank/tapes`, bind-mounted to `/var/lib/tapes` in the CT |
| D8 | Scope M0–M2 | **debra only** |

## Hard guardrails (every agent, every task)

- **Never invoke `ark`.** If an ark run is wanted, tell the human.
- **No `rm`, ever, anywhere** — not on debra, not in the CT, not in tests. Move
  scratch to `/tmp`. The archive opens source trees **read-only**.
- **No force-push / no history rewrite** on any remote. Fast-forward only; if a
  push isn't a fast-forward, stop and report.
- **No Proxmox mutations outside task T4**, and T4 requires the human present.
- **No global `pip`/`npm` installs.** Any new system dependency (brew, apt) is
  escalated to the human first. Cargo crates are fine (crates.io reachable).
- Do not touch `~/Code/regina/` — read it, copy from it, never edit it.

## Repo layout (create in T1)

```
Cargo.toml                 # workspace, one binary crate: src/bin? no — one bin, subcommands
src/main.rs                # clap: collect | ingest | serve | audit | verify | keygen
src/scan.rs                # source discovery: claude / codex / pi roots, quiescence window
src/manifest.rs            # blake3 hashing, session-set grouping, state.db
src/store.rs               # zstd + age blob write/read, fsync+rename, content-addressed
src/transport.rs           # russh + russh-sftp push, have/haven't handshake
src/db.rs                  # rusqlite + bundled-sqlcipher, schema from PLAN.md §4
src/parse/{mod,pi,claude,codex}.rs   # → canonical Session/Turn/ToolCall
src/render/{mod,tmpl}.rs   # canonical model → HTML; HTML_TMPL/CSS/JS copied from
                           # ~/Code/regina/scripts/render_trace.py (keep header attribution)
src/web.rs                 # axum: / list, /s/{host}/{id} session page
```

Crate list (pre-approved, all on crates.io): `clap`, `serde`, `serde_json`,
`blake3`, `zstd`, `age`, `russh`, `russh-sftp`, `rusqlite` (feature
`bundled-sqlcipher`), `axum`, `tokio`, `anyhow`, `thiserror`, `chrono`,
`walkdir`, `sha2`(unused if blake3 covers it), `tracing`+`tracing-subscriber`.
`askama` optional — the Python renderer is string-concatenation, so hand-rolled
`String` pushing is a faithful port and avoids a template engine.

## Work units

**T1 — scaffold + `tape scan` (PLAN M0).** Workspace, clap skeleton, `scan`
walks the three roots, prints manifest (source path, tool, session id, size,
mtime, blake3). Writes nothing outside `~/.local/share/tapes/`.
*Done when:* counts match PLAN §2 — 99 Claude top-level sessions / 205 jsonl,
348 Codex, 291 pi — and Claude subagent dirs group under their parent session.

**T2 — crypto + blob store (PLAN M1). LANDED on branch `t2-store` (2026-10-31).**
`tape keygen`, `tape store`, `tape verify` are real: age X25519 + zstd-12,
blobs at `store/blobs/<blake3-of-plaintext>.tape`, write→fsync→rename, no
delete/rewrite path anywhere, 16-way parallel. 9 tests in `tests/store_tests.rs`.
Measured: 846 blobs, 2402 MiB → 891 MiB (**2.69×**), `verify` 846/846 ok.
SIGKILL mid-store → 337 blobs, all readable, rerun filled the remaining 11,
final verify 348/348.
Three structural changes T1 must know about: `src/lib.rs` added (crate is now
lib+bin so `tests/` can link it), `Tool` gained `Default` (`Pi`), and
`store::walk_for_verification` is an explicit **stopgap** file-lister to delete
once `tape scan` lands.

**Human-only sub-step (still open):** generating the *production* age identity
and backing it up into `pass` (`pass insert tapes/identity.age`) is interactive
gpg — the human does it. Everything above used throwaway keys under /tmp.

**T3 — parsers + index (PLAN M3).** Canonical model, three parsers, SQLCipher
schema from PLAN §4. Watch the pi timestamp trap: envelope `timestamp` is
ISO-8601, inner `message.timestamp` is **epoch-ms** (use the `>1e11` ⇒ ms
heuristic that `render_trace.py` already needed). Codex: only `response_item`
feeds turns; `session_meta`/`turn_context`/`token_usage_record` become index
columns, everything preserved in the blob.
*Depends on:* T1. *Parallel with:* T2.
*Done when:* index row counts reconcile to blob counts; 5 sessions per tool
hand-checked against the raw jsonl.

**T4 — CT 127 + push + ingest (PLAN M2).** The only infra task. Follow
`~/homelab/docs/ADDING-A-NEW-SERVICE.md` steps 1–2, **skip 3–4**.
Verified facts: proxmox `rk@<proxmox-lan-ip>` (key auth, `sudo -n` works, `rk` is
uid 1000); highest existing CT is **126**; template
`local:vztmpl/debian-13-standard_13.1-2_amd64.tar.zst` is present; `tank` is a
filesystem dataset on the RAIDZ2 pool with **33 TiB free** (host path
`/tank/tapes`); debra's sshd listens on :22; debra's key is `id_ed25519`.
Sequence: create CT → `rk` + key → apt build deps + rust → build in-CT →
`zfs create tank/tapes` + bind mount → ingest → **then** firewall egress cut
(allow LAN only). Verify the CT still ingests and serves after the cut.
*Depends on:* T1, T2, T3. *Human present required.*

**T5 — renderer port + `tape serve` (PLAN M4).** Port
`~/Code/regina/scripts/render_trace.py` to Rust. Keep its CSS/JS/HTML verbatim:
density modes (skim/read/full), one-turn-at-a-time paging, resizable timeline
rail, emoji-per-tool badges (`bash_chain` verb mapping), edits rendered as
old/new diffs not JSON, output collapse >1400 chars, repeats callout. axum
serves list + session pages bound to the LAN address, port 8799.
*Depends on:* T3. *Done when:* PLAN M4 acceptance check passes for one session
of each tool.

**T6 — launchd + `audit` (PLAN M5).** LaunchAgent
`net.robertkarl.tapes.plist` (mirror the existing
`~/Library/LaunchAgents/net.robertkarl.regina-brain-scheduler.plist` pattern;
hourly, `StandardOutPath`/`StandardErrorPath` into `~/Library/Logs/tapes/`).
`audit` diffs source-vs-archive and appends vanished session IDs to the DB —
this is the instrument that proves or kills the pruning hypothesis.
*Depends on:* T4.

## Parallelization (updated after the T1+T2 merge)

**T1 and T2 are merged into `main` and measured end to end.** Next up:
**T3 (parsers + SQLCipher index) is unblocked and is the natural next unit** —
it consumes `Manifest`/`FileEntry` and the blob store, and touches no infra.
**T4 stays serialized with the human present.** T5 after T3, T6 after T4.

Worktree recipe for T3 (one checkout per unit; the shared build cache is fine
for third-party deps, but remember the caveat below about `debug/tape`):

```bash
cd ~/Code/claude-transcript-archiver
git worktree add ../claude-transcript-archiver-t3 -b t3-index
export CARGO_TARGET_DIR=$HOME/Code/tape-target
```

## Current state of the repo (merged, 2026-10-31)

> **See `STATE.md` for the authoritative snapshot** (measured numbers, Proxmox
> half-state, reusable blob fixture, API surface, gotchas). This section is the
> task ledger.

`main` holds scaffold → `t1-scan` → `t2-store` → integration (`cf9055c`).
Branch labels `t1-scan`/`t2-store` and the T1 worktree were removed after the
merge; the history lives in the merge commits.

Working, measured on the real transcripts:

- `tape scan` — 4 roots, 853 files, per-tool counts exact against PLAN §2
  (pi 292 / claude 205 / codex 356), 14 Claude groups carrying subagents,
  7 non-quiescent, anomalies reported not filtered; manifest written atomically;
  `--json` pipeable on stdout.
- `tape keygen` / `store` / `verify` — 846 blobs, 2387 MiB → 888 MiB (2.69×),
  verify 846/846, 0 failures; SIGKILL mid-store leaves only complete blobs.
- 16 tests green (7 in `scan.rs`/`manifest.rs`, 9 in `tests/store_tests.rs`).

Not implemented: `tape push`, `ingest`, `serve`, `audit` (T4/T5/T6).

## Decisions made without asking (none touch encryption at rest or in transit)

1. **`age` stays at 0.11.5 — X25519, no ML-KEM.** age 0.12's post-quantum layer
   cannot coexist with russh 0.65 in one dependency graph. X25519 has no
   practical break; PQ is a harvest-now-decrypt-later hedge, and this archive's
   threat model is a stolen disk on a LAN. In transit is unaffected: russh still
   negotiates PQ key exchange with the SSH server. Revisit as a ticket if the
   archive ever leaves the LAN. Not a blocker.
2. **Archive unit stays `.jsonl` session files only.** The pi `…jsonl.bak`
   (21.9 MB), `sessions.sqlite` and 46 `.lock` files are pi's own working state,
   not transcripts — the `.bak` is a duplicate of a session we already hold.
   `~/.claude/history.jsonl` is prompt history, not a transcript; PLAN listed it
   as a bonus artifact and that is deferred until it costs something. Adding a
   root later is a one-line change in `scan::source_roots()`.
3. **Port 8799, LAN bind, no auth.** Locked; it is the default in `tape serve`.
   Not exposed anywhere beyond the LAN until the egress/listener rules say so.
4. **The 11 empty pi cwd dirs are data, not a question.** `audit` (T6) diffs
   source-vs-archive nightly and logs vanished ids; that is how they get
   explained or dismissed. No action now.

## What actually needs a human — two things

1. **The production age identity.** Generate it on the archive host, then back
   it up (`pass insert tapes/identity.age`, interactive gpg). This is the only
   irreversible act in the project: lose that file and 2.4 GB of archive is
   permanently unreadable. Everything else is rebuildable or reversible.
2. **T4, with you at the keyboard.** `pct create 127`, `zfs create tank/tapes`,
   and the CT firewall egress cut are mutations on shared infrastructure that
   an agent should not make alone.

Everything else in this project is agent-solvable: T3 (parsers + index) is
unblocked and touches no infra; T5/T6 follow.

## Build gotchas (learned the hard way)

- **`age` is pinned to 0.11.5, not 0.12.** age 0.12 pulls `ml-kem 0.2.x`, which
  needs `kem =0.3.0-pre.0` and cannot coexist with russh 0.65's `ml-kem 0.3.3`
  (`kem ^0.3` → 0.3.0). With age 0.12 the build dies inside `ml-kem`. Comment in
  `Cargo.toml` says so; do not bump it blind.
- **An interrupted build leaves a stale relinked binary.** After I killed a
  build, `target/debug/tape` kept being *relinked from an old artifact* — the
  CLI silently reverted to the pre-edit stubs while the source was correct.
  Symptom: `tape store --help` shows no options. Fix: `cargo clean -p tape`.
  Never trust the binary; check `--help` after a build that was interrupted.
- **A shared `CARGO_TARGET_DIR` means one shared `debug/tape`.** With two
  worktrees building the same package into the same target dir, the binary
  belongs to whichever branch built last — it looked like the CLI had silently
  reverted to old stubs. Build and run in one command (`cargo run -- …`), and
  check `--help` before trusting a binary from a shared target dir.
- Cold dependency build is ~361 crates (russh, axum, bundled sqlcipher): about
  6 minutes on this machine. Warm rebuilds are seconds.

## Open items the human still owes

- The age keypair generation + `pass` backup (T2, interactive).
- Being present for T4 (sudo on shared Proxmox + a firewall change).
- `tape serve` port confirmation (8799 assumed).
