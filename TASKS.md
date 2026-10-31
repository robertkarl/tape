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
| D7 | Storage | CT 127 on proxmox .57; data on the **spinning RAIDZ2 pool `tank`** → host path `/tank/tapes`, bind-mounted to `/var/lib/tapes` in the CT |
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

**T2 — crypto + blob store (PLAN M1).** `tape keygen`, `tape store`,
`tape verify`. age X25519, zstd level ~12, blob = `store/blobs/<blake3>.tape`,
write→fsync→rename, `verify` decrypts+decompresses+rehashes every blob.
*Done when:* `tape verify` = 0 failures over all ~2.4 GB, and a killed-mid-write
run leaves no unreadable blob.
**Human-only sub-step:** generating the age identity and backing it up into
`pass` (`pass insert tapes/identity.age`) is interactive gpg — the human does it.
Agents generate a *test* keypair in `/tmp` only.

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
Verified facts: proxmox `rk@192.168.50.57` (key auth, `sudo -n` works, `rk` is
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

## Parallelization

Safe to run **concurrently right now**: T1, T2 (separate modules, no shared
files; T1 first if you want the manifest types settled — T3 imports them).
Serialize: **T4 alone, human present.** T5 after T3. T6 after T4.

Fastest useful path: T1+T2 in parallel → T3 → **T4 (this is the milestone that
ends the data-loss risk)** → T5 → T6.

## Current state of the repo (2026-10-31)

Git repo initialized on `main`. Scaffold committed: workspace `Cargo.toml` with
the full pre-approved dependency set pinned to current crates.io versions,
`Cargo.lock` resolved (361 crates — resolution confirms `bundled-sqlcipher` is
a valid rusqlite feature), all module stubs compiling-shaped with
`bail!("not implemented (Tx)")`, and `src/types.rs` + `src/paths.rs` as the
shared contract so parallel branches cannot invent competing types.

**Not yet compile-checked.** The first `cargo check` is a cold build of russh +
axum + sqlcipher and runs many minutes; it was started and cancelled to avoid
the wait. Whoever picks up a task should kick it off once, early.

Worktrees (run these, then start one pi session per worktree):

```bash
cd ~/Code/claude-transcript-archiver
git worktree add ../tape-wt/t1-scan -b t1-scan
git worktree add ../tape-wt/t2-store -b t2-store
# share one build cache across all three checkouts; cargo takes a lock and
# serializes concurrent builds. Third-party deps build once; the local crate
# still rebuilds per branch when you switch sessions.
echo 'export CARGO_TARGET_DIR=$HOME/Code/tape-target'   # set in each session
```

Ready-to-paste agent briefs: **`prompts/T1.md`**, **`prompts/T2.md`**.

## Open items the human still owes

- The age keypair generation + `pass` backup (T2, interactive).
- Being present for T4 (sudo on shared Proxmox + a firewall change).
- `tape serve` port confirmation (8799 assumed).
