# STATE — snapshot for the T4 session (2026-10-31, debra)

Read with `SPEC.md` → `PLAN.md` (esp. **§2.1 the working subset**) → `TASKS.md`
→ `prompts/T3.md`. Every number here was measured on this machine, not assumed.

Repo: `~/Code/claude-transcript-archiver`, branch `main`, clean, **pushed to
`origin` = https://github.com/robertkarl/tape (public)**. No force-push, ever;
pushes must be fast-forward.

## Status by unit

| unit | state |
|---|---|
| T1 `scan` | done, merged |
| T2 `keygen/store/verify` | done, merged |
| **T3 parsers + SQLCipher index + `ingest`/`stats`** | **done, merged, acceptance met** |
| **T4 CT 127 + `push`** | **next.** Infra clean and ready; `src/transport.rs` is still a 28-line stub |
| T5 renderer port + `serve` | not started, depends on T3 (unblocked) |
| T6 launchd + `audit` | not started, depends on T4 |

37 tests pass (23 unit, 5 index integration, 9 store).

## T3 acceptance, measured

Fresh run in `TAPE_HOME=/tmp/tape-t3-full`:

| stage | pi | claude | codex | total |
|---|---|---|---|---|
| `scan` found | 292 | 205 | 356 | 853 |
| quiescent (archivable) | 291 | 201 | 356 | 848 |
| skipped, still being written | 1 | 4 | 0 | **5** |
| blobs stored | 291 | 201 | 356 | **848** |
| `blobs` rows | 291 | 201 | 356 | **848** |
| `sessions` rows | 291 | **96** | 356 | 743 |

`messages` 94,428 · `tool_calls` 56,075 · sidechain turns 14,871 ·
0 parse failures · ingest 75 s · **re-ingest: `ingested: 0`, row sum
152,094 → 152,094**. Claude 201 files → 96 sessions is the group rule
(subagent files share the parent's `session_id`), not lost data.

15 hand spot-checks (5 per tool, largest sessions) against an independently
written Python model — per-blob turn counts, role histograms, token totals,
session span — **0 mismatches**. That check found two real bugs, both fixed:
per-file `DELETE` clobbering Claude group turns (fixed by a `blob` column on
`messages`/`tool_calls`), and session span ignoring lines that never become
turns.

### One loose end, deliberately not closed yet

Parsers count `unmapped_lines` but that number **never reaches the index**;
`parse_failures` is wired but effectively unreachable because the parsers are
tolerant of unknown line types. So a future format change would silently shrink
the index while Layer 1 stays byte-complete. Fix is ~10 min: `unmapped_lines
INT` on `sessions`, written by `ingest`, surfaced in `tape stats`. Worth doing
before T4 goes live.

## D9 — the working subset is `~/Code/ubdogs` (decided 2026-10-31)

From now on, archive **only sessions whose recorded cwd is
`/Users/robertkarl/Code/ubdogs`**. Measured at decision time:

| tool | match | files |
|---|---|---|
| pi | dir `~/.pi/agent/sessions/--Users-robertkarl-Code-ubdogs--` | **8** (3.2 MB) |
| codex | first line `session_meta.payload.cwd == /Users/robertkarl/Code/ubdogs` | **2** |
| claude | dir `~/.claude/projects/-Users-robertkarl-Code-ubdogs` | **0 — does not exist** |

**The user expected Claude transcripts here; there are none.** Three Claude
files mention ubdogs as a cwd mid-session but belong to sessions started in
`~/Code` and `~/Code/regina`; they are excluded. Expect pi + codex only until
Claude is actually used in that project.

Mechanism (first step of T4): `tape scan --only <path>` filters at scan time
and **records the filter in the manifest**, so `store`/`push`/`ingest`/`audit`
inherit it. Exact match on the tool's own cwd encoding — never a substring of
the path slug, which would drag in `ubdogs-api` and worktrees. For Codex, read
line 1 and compare `payload.cwd`. Filtering happens only at scan: nothing
downstream deletes a blob that falls out of the subset.

## T4 — what is ready, and what to do in order

**Infra: clean slate, verified.**
- CT 127 is **gone** — Proxmox rolled back the aborted create (`pct config 127`
  → does not exist, no `subvol-127` dataset). Nothing to clean up.
- `tank/tapes` exists, mountpoint `/tank/tapes`, 33 T available, owned
  `100000:100000` so an unprivileged CT can bind-mount it:
  `--mp0 /tank/tapes,mp=/var/lib/tapes,backup=0`.
- Template `local:vztmpl/debian-13-standard_13.1-2_amd64.tar.zst` present.
- Rootfs syntax that works: `--rootfs tank-vm-disks:vm-127-disk-0,size=8G`
  (passing `subvol-127-disk-0` explicitly fails).
- `local-lvm` is **99.89% full** — never put a rootfs there.
- House conventions: unprivileged, `ip=dhcp`, `firewall=1` on net0,
  `features nesting=1`. Agreed sizing: 1024 MB + 2 GB swap + 2 cores for the
  build, then drop to 512 MB.

**Code gap: `push` does not exist.** `src/transport.rs` =
`bail!("not implemented (T4): sftp push of missing blobs")`. russh and
russh-sftp have never moved a byte.

**Order of work:**
1. `scan --only <path>` + filter recorded in the manifest (D9).
2. Implement `push`, then **test it against debra's own sshd**
   (`127.0.0.1:22`, your `id_ed25519`, a temp target dir). Zero Proxmox
   mutation; exercises key auth, SFTP, resume-missing-blobs.
3. Human-present CT session: `pct create 127` → `rk` user + debra's pubkey →
   `apt install build-essential cmake pkg-config perl` + rust → `git clone
   https://github.com/robertkarl/tape` (public now, so no scp) → build →
   `tape keygen` on the CT → push subset blobs → `ingest` → verify →
   **then** the firewall egress cut (LAN only).

**Design decisions already made for T4 (object if you disagree):**
- `push` writes straight to `blobs/<hash>.tape` (temp → fsync → rename). Names
  are content hashes, so staging/quarantine buys nothing; `ingest` re-verifies
  the hash on decrypt anyway.
- The **manifest travels with the blobs** — `ingest` needs it for
  path/tool/session-id ↔ hash, and `manifest.host` is what makes a second
  machine's archive land in the same index.
- `tape keygen` **on the CT**: the age identity never leaves it; pubkey goes to
  debra. `db.key` is generated on the CT at first `ingest`. No `pass` ceremony:
  this archive is re-importable from the tools' own trees, single user.
- Auth: debra's existing `~/.ssh/id_ed25519` → `rk` on the CT.

**Risk to budget for:** russh pulls **`aws-lc-sys`**, which builds with CMake —
the CT needs `cmake` (and `build-essential`, `pkg-config`, `perl`) before
`cargo build`, and aws-lc + bundled SQLCipher on 2 cores is a **10–20 minute**
compile, not the 6 minutes it takes on debra.

## Reusable fixtures (all under /tmp, none deleted)

- `TAPE_HOME=/tmp/tape-t3-full` — 848 blobs, verified, **indexed** (current parsers).
- `TAPE_HOME=/tmp/tape-merged` — 846 blobs, indexed with pre-span-fix parsers;
  that stale index is at `state.db.pre-span-fix`, moved aside not deleted.
- Spot-check script: `/tmp/tape-spotcheck.py` (dev-only, not in the repo — the
  pipeline stays 100% Rust).

## Build gotchas, learned the hard way

- Shared `CARGO_TARGET_DIR=$HOME/Code/tape-target` → **one shared `debug/tape`**;
  with several worktrees it belongs to whichever branch built last. Build and
  run in one command; check `tape --help` before trusting a binary.
- An interrupted build can leave a **stale relinked binary**. Fix:
  `cargo clean -p tape`.
- `git commit -m "…"` with backticks runs command substitution — quote messages
  with single quotes.
- Cold dep build ≈361 crates ≈6 min on debra; warm rebuilds are seconds.
- `age` pinned to 0.11.5: age 0.12's `ml-kem 0.2` cannot coexist with russh
  0.65's `ml-kem 0.3.3`. Do not bump blind. Comment in `Cargo.toml` says so.

## Human-only items

1. Being present for the CT session (Proxmox sudo + firewall egress cut).
2. Optional: `pass insert tapes/identity.age` once the CT has a real identity.

## Findings worth keeping

- **Paths move under us.** A Codex file archived at `~/.codex/sessions/2026-10-28/…`
  now lives under `~/.codex/sessions/2026/10/28/…`. Content hashes are stable;
  paths are not. `audit` (T6) must key on `(session_id, hash)`, never path.
- **Manifest drift is normal:** mid-run the tools write new versions, so
  `ingest` reports `missing blob` for entries whose hash was superseded. Reported,
  not fatal.
- 7 sessions legitimately have **zero turns** — those files really are
  header/bookkeeping only. Not a parser bug.
- Older Codex builds wrote no `token_usage_record`, so those sessions have
  `in/out_tokens = 0`. The data exists as `payload.token_count`; follow-up, not
  a loss — the blob holds everything.
- Claude's oldest session is 2026-07-18 with a suspiciously thin tail while
  `cleanupPeriodDays` is unset (default 30). `tape audit` is the instrument that
  proves or kills that.
