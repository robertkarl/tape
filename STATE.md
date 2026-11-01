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
| **T4 CT 127 + `push`** | **done except the firewall cut, deliberately deferred until after T5.** CT `tapes` (127) is up at 192.168.50.108 and holds the first real archive |
| **T5 renderer port + `serve`** | **next.** Brief in `prompts/T5.md`; stubs only, deps already present |
| T6 launchd + `audit` | not started, depends on T4 |

42 tests pass (28 unit, 5 index integration, 9 store).

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

Implemented: `tape scan --only <path>` / `tape push --only <path>`. The filter
is **written into the manifest** (`manifest.only`), so `store`, `push`,
`ingest` and `audit` inherit it instead of re-deciding it, and the archive host
can tell a subset scan from a whole machine. Matching is per-tool and exact:
pi by session-dir slug (`--Users-robertkarl-Code-ubdogs--`), Claude by project
slug (`-Users-robertkarl-Code-ubdogs`), Codex by reading `session_meta.cwd`
from the file (its tree is dated, not slugged). A Codex file with no readable
cwd is reported as an anomaly, never silently dropped.

Measured against the live machine — matches the hand measurement exactly:

```
$ tape scan --only ~/Code/ubdogs
subset (D9): sessions with cwd exactly /Users/robertkarl/Code/ubdogs
pi         8 files       3368084 bytes
codex      2 files        272297 bytes
claude     0 files             0 bytes
total: 10 files
```

**There is no Claude transcript on this machine for that project.** Three
Claude files mention ubdogs as a cwd mid-session but belong to sessions started
in `~/Code` and `~/Code/regina`; they are excluded. Expect pi + codex only
until Claude is actually used there — which also means the first real push
exercises **no** Claude subagent-grouping code.

## T4 — code is built and measured; the CT is the remaining half

### What was built

`src/transport.rs` (push over russh + russh-sftp, in-process),
`scan --only` (D9), `manifest::load_all` (one manifest per host,
newest wins), `ingest::ingest_all` (indexes every host's manifest into one
SQLCipher index), `tape push` CLI (`--key`, `--remote-home`, `--only`,
`--dry-run`), tape-owned TOFU `known_hosts`.

### Measured end to end against debra's own sshd (loopback, 2026-10-31)

No Proxmox mutation, real OpenSSH server, real SFTP:

| step | result |
|---|---|
| `scan --only ~/Code/ubdogs` | pi 8 + codex 2 + claude 0 = 10 files, 3.6 MB |
| `store` | 10 blobs, **3.68×**, 992 KB, 0 failures |
| `verify` | 10/10 ok |
| `push --dry-run` | connects, TOFU-records the host key, `to send: 10` |
| `push` | 10 blobs landed under `store/blobs/`, manifest at `manifests/debra.json`, **0 temp debris** |
| `push` again | `already present: 10, to send: 0` — idempotent |
| remote `ingest` | 10 blobs → 10 sessions, 818 messages, 735 tool calls, host `debra`, 0 parse failures |
| remote `ingest` again | `ingested: 0, already indexed: 10` |
| **tamper a remote blob, push again** | blob **not rewritten** (size still shows the injected bytes) |
| **ingest the tampered store** | `read/decrypt fails: 1 … decryption error`, other 9 index fine, **exit code 1** |
| **wrong host key in known_hosts** | refused: `host key … does NOT match … The server key changed at line 1` |
| **client key not in authorized_keys** | `authentication failed … is this public key in its authorized_keys?` |

Reproduce:

```sh
export TAPE_HOME=/tmp/tape-t4-subset
tape scan --only ~/Code/ubdogs && tape keygen && tape store && tape verify
tape push 127.0.0.1 --user robertkarl --key ~/.ssh/id_rsa \
     --remote-home /tmp/tape-t4-remote --only ~/Code/ubdogs
cp $TAPE_HOME/identity.age /tmp/tape-t4-remote/   # test only: on the CT the
                                                  # identity is generated there
TAPE_HOME=/tmp/tape-t4-remote tape ingest && TAPE_HOME=/tmp/tape-t4-remote tape stats
```

Note `--key ~/.ssh/id_rsa`: on debra, `id_rsa` is in `authorized_keys` and
`id_ed25519` is **not**. That only matters for this loopback test — on the CT we
write its `authorized_keys`, and `tape push` defaults to `id_ed25519`.

### The archive host as built (2026-10-31, with the human present)

| | |
|---|---|
| CT | `127 tapes` on proxmox, unprivileged, Debian 13, 2 cores / 1024 MB / 2 GB swap, `onboot 1`, `ip=dhcp`, `firewall=1` |
| rootfs | `tank-vm-disks:subvol-127-disk-0,size=8G` — 7.7 G free |
| archive dataset | `mp0: /tank/tapes,mp=/var/lib/tapes,backup=0` — 34 T |
| user | `rk` inside the CT (uid 1000 → **101000 on the host**; `/tank/tapes` chowned to it) |
| age identity | `/var/lib/tapes/identity.age`, mode **0400**, generated **on the CT**, never copied out. debra holds only `pubkey.txt` |
| index key | `/var/lib/tapes/db.key`, mode **0400**, auto-generated at first ingest |
| toolchain | **rustup stable 1.99.1** — Debian 13's `rustc 1.85` is too old: russh and rusqlite declare `rust-version = 1.89` |
| apt deps | `build-essential cmake pkg-config perl git curl ca-certificates` **and `libssl-dev`** |
| build | `cargo build --release` ≈ 4 min total on 2 cores (2 min to the sqlcipher failure + 1.5 min after the fix) |
| tests on x86_64 | 41 passed (27 + 5 + 9) |
| host key | `SHA256:FzrV7W/N1YJziYdrgk4YlQ6RXsUyLyoi0k3aCXcuDV4` — recorded TOFU by tape, then **verified against the CT's own `ssh_host_ed25519_key.pub`** |
| first archive | 10 blobs (pi 8 + codex 2), verified 10/10 on the CT, ingested: 10 sessions / 818 messages / 735 tool calls, 0 parse failures |
| re-push | `already present: 10, to send: 0`; re-ingest `ingested: 0, already indexed: 10` |

**Three things the real run corrected in this file:**

1. **Rootfs syntax.** `--rootfs tank-vm-disks:vm-127-disk-0,size=8G` does *not*
   work — that form names an existing volume. `tank-vm-disks:8` creates a new
   8 G one. (Earlier note here said the opposite; it was wrong.)
2. **`pct create` over ssh hangs** waiting for a root-password prompt. Feed it
   `</dev/null`, or pass `--ssh-public-key <file>` — and note the key argument
   is a **file path or URL, not inline key data**.
3. **`tape verify` only works on the archive host.** It needs the private
   identity, which by design never leaves it. On a source host the sequence is
   `scan → store → push`; verification is the archive host's job.

**Pushed manifests are history, not a file that gets replaced.** The first
re-push failed: SFTP `rename` will not overwrite an existing name. Rather than
force it, `push` now writes `manifests/<host>-<YYYYmmddTHHMMSSZ>.json`, one per
scan, and `load_all` selects the newest name per host without parsing the rest.
That history is what `tape audit` (T6) will read to answer "what did the tools
hold at time T" — a log we do not want to truncate.

Left behind by that bug, inert and ignored (they do not end in `.json`):
`/var/lib/tapes/manifests/debra.json.tmp-{32367,32397,32688,32714}`.

### Remaining T4 work

**Infra is clean and verified:** CT 127 is gone (Proxmox rolled back the aborted
create — `pct config 127` does not exist, no `subvol-127` dataset). `tank/tapes`
exists, mountpoint `/tank/tapes`, 33 T available, owned `100000:100000` for an
unprivileged bind mount (`--mp0 /tank/tapes,mp=/var/lib/tapes,backup=0`).
Template `local:vztmpl/debian-13-standard_13.1-2_amd64.tar.zst`. Rootfs syntax
that works: `--rootfs tank-vm-disks:vm-127-disk-0,size=8G` (explicit
`subvol-127-disk-0` fails). `local-lvm` is **99.89% full** — never there.
Conventions: unprivileged, `ip=dhcp`, `firewall=1` on net0, `features nesting=1`.
Sizing: 1024 MB + 2 GB swap + 2 cores for the build, then drop to 512 MB.

What is left of T4, in order:

1. **The firewall egress cut — decided: AFTER T5.** Not applied yet. Proposed
   CT-level rules (`/etc/pve/firewall/127.fw`): `policy_in DROP`,
   `policy_out DROP`, allow in from `192.168.50.0/24` for 22 and 8799. Until it
   is cut, the CT can `git pull` and `cargo build`, which is how T5 gets
   deployed. **Cutting it is a human-present task.**
2. **Sizing — stays at 1024 MB until T5 is built**, then revisit 512 MB.
   Linking `aws-lc-sys` + bundled SQLCipher in 512 MB is a coin flip.
3. **T6 launchd** will make the push hourly; nothing about it is blocked.
4. Cosmetic: `/var/lib/tapes/manifests/debra.json.tmp-{32367,32397,32688,32714}`
   are inert debris from the pre-fix push (they do not end in `.json`, so the
   loader ignores them). Delete only on request.

## T5 — ready to start

Brief: **`prompts/T5.md`**. Readiness facts:

- `src/render/mod.rs` and `src/web.rs` are stubs; `axum` + `tokio` are already
  dependencies. Nothing else needs to exist first — T3's canonical `Session`
  is the renderer's input.
- **Deploy loop for the CT:** `git push origin main` from debra, then on the CT
  `cd ~/tape && git pull --ff-only && cargo build --release` (egress still
  open — that is exactly what the deferred firewall cut buys).
- **The CT index has no Claude sessions** (the D9 subset has none), so PLAN M4's
  acceptance — "the Claude page renders subagents" — must be taken on debra
  against `TAPE_HOME=/tmp/tape-t3-full` (96 Claude sessions, 14 groups with
  subagents). Do not widen the subset for a nicer demo; that is the human's call.
- Renderer source to port: `~/Code/regina/scripts/render_trace.py`, 915 lines,
  `HTML_TMPL` from line 646. Lines 84–208 (`find_context_files`,
  `injected_context`, `find_meta`) read a live source repo and are **not**
  portable — the index is the only input on the archive host.

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
