# T1 report — `tape scan` (branch `t1-scan`, commit `0c633f5`)

Owned files changed: `src/scan.rs`, `src/manifest.rs`. Nothing else in the repo
was touched. No push, no merge.

Worktree: `~/Code/claude-transcript-archiver-t1-scan` (branch `t1-scan`,
created with `git worktree add ../claude-transcript-archiver-t1-scan -b t1-scan`
from `803f920`). Shared build cache: `~/Code/tape-target`.

## Measured vs expected

| Tool | Expected (PLAN §2) | Measured | Verdict |
|---|---|---|---|
| Claude | 205 jsonl / 99 top-level / ~827 MB | **205 files / 99 groups** / 856,538,161 B (816.9 MiB) | counts exact |
| Codex | 348 + 8 archived / ~1.4 GB | **356** (348 + 8) / 1,544,031,731 B (1.44 GiB) | exact |
| pi | 291 files / 84 dirs / ~142 MB | **292** files / 83 dirs (72 with jsonl) / 127,528,495 B jsonl-only | +1 file, explained |

Other measured facts: **anomalies: 0 over 853 files**; **14 Claude groups carry
subagents** (106 subagent files, all with a sibling main file — no orphans
today); **4 non-quiescent** (live sessions, including the one running this task).

### Reconciliations (investigated, not filtered away)

- **pi 292 vs 291** — two pi sessions carry today's date; one
  (`2026-10-31T21-31-01-061Z_01a193df….jsonl`, 14:31 local) already existed when
  PLAN.md landed at 14:50. The extra file is *this work's own session*.
- **pi 84 vs 83 dirs** — PLAN counted `find -type d`, which includes the root.
  Only **72** dirs actually contain a `.jsonl`.
- **pi 142 MB vs 121.6 MiB** — PLAN measured the whole tree
  (149,487,008 B = 142.6 MiB): a **21.9 MB `…jsonl.bak`**, 46 `.lock` files and
  `sessions.sqlite`. My number is jsonl-only, per the `.jsonl` filter.
- **Claude 827 MB vs 816.9 MiB** — PLAN's figure is `du` (block-allocated);
  mine is the sum of exact byte sizes.

### Findings — files/things that broke an assumption

1. **11 pi cwd dirs are completely empty** (no jsonl, no `.lock`): 83 dirs, only
   72 populated. Possible historical deletion — direct grist for T6's `audit`
   (`vanished`) work.
2. **pi filename drift**: 39 files end `_<id8>`, 253 end `_<full-uuid>`. The
   line-1 `{"type":"session","id":…}` header is authoritative, so parsing is
   unaffected; the filename id8 is only a fallback.
3. **One 21.9 MB `…jsonl.bak`** in `~/.pi/agent/sessions/--Users-robertkarl-Code-
   glass-slipper--/` (2026-07-22), plus 46 `.lock` files and a `sessions.sqlite`.
   All skipped by the `.jsonl` filter. **Open question for the human:** should the
   `.bak` and the pi sqlite index be archived as bonus artifacts (PLAN does the
   same for `~/.claude/history.jsonl`)?
4. **Claude first lines are not message records** (`type: mode`,
   `permission-mode`, `atis-latch`, …). Per the brief the session id comes from
   the filename stem; the header is not used.
5. **Codex: all 356 files** have a parseable `session_meta.payload.id` that is a
   uuid, and it always matches the filename tail. No fallbacks were needed in
   practice; the fallback path exists and is unit-tested.
6. Claude subagent dirs sit at exactly two depths — measured, not assumed:
   `<enc-cwd>/<uuid>.jsonl` (99) and `<enc-cwd>/<uuid>/subagents/agent-*.jsonl`
   (106). Any other depth is reported as an anomaly (and still grouped).

## Blockers — outside T1's ownership, need the human

1. **The workspace cannot compile as scaffolded (dependency graph).**
   `age 0.12.1` → `ml-kem 0.2.1` (requires `kem ^0.3.0-pre.0`), while
   `russh 0.65` → `ml-kem 0.3.3` (requires `kem ^0.3`). Cargo unifies `kem` to
   **0.3.0 stable**, whose trait signatures broke `ml-kem 0.2.1` — 18 compile
   errors (`E0437`, `E0407`, `E0107`, `E0053`, `E0046`). No `ml-kem` 0.2.x works
   against `kem` 0.3.0 stable (0.2.2/0.2.3 pin `=0.3.0-pre.0`), and `age` 0.12.1
   is the newest `age` release with no feature to disable ml-kem.
   **Cheapest fix: `age = "0.11.5"`** — it has no `ml-kem` dependency at all
   (X25519 only, i.e. no post-quantum layer). Alternatives: drop russh's
   post-quantum KEX, or `[patch.crates-io]`. `Cargo.toml`/`Cargo.lock` are not
   mine, so nothing was changed. The T2 agent's build hit the identical failure.
2. **Two pre-existing compile errors in other units' files**:
   `src/web.rs:6` — unescaped `{host}/{id}` inside `anyhow::bail!` (two
   `E0425` format-capture errors); `src/parse/mod.rs:15` — `#[derive(Default)]`
   on a struct holding `crate::types::Tool`, which has no `Default` (`E0277`).
3. **`tests/scan_test.rs` cannot work**: this is a bin-only crate (no lib
   target), so an integration test cannot `use tape::…`. Adding a lib target
   means touching `Cargo.toml`/`main.rs`. I moved the integration test I had
   written to `/tmp/tape-t1-retired/scan_test.rs.integration` (no `rm`) and
   ported its coverage into `#[cfg(test)] mod tests` inside my two files:
   **7 tests, all passing.**
4. **`main.rs` prints only `"853 entries"`.** The human-readable per-tool summary
   is emitted from `scan()` via `tracing::info!` (stderr) so that
   `tape scan --json` stays pipeable on stdout. Getting it on stdout needs a
   `main.rs` line, which is not mine to make. `scan::summarize(&ScanResult)` is
   public and ready for it.

## How the work was verified despite blocker 1+2

In a throwaway `/tmp` copy of the worktree (`/tmp/tape-t1-check`) with **only**
those three blockers shimmed (age/russh/russh-sftp removed from the copy's
`Cargo.toml`; the two stub errors patched in the copy):

- `cargo test --bin tape` → 7 passed
- `cargo clippy --bin tape` → 0 warnings attributable to `scan.rs`/`manifest.rs`
- `tape scan` → real summary + manifest written to `TAPE_HOME`
- `tape scan --json` → clean JSON on stdout

The copy's `src/scan.rs` / `src/manifest.rs` were byte-identical to the
worktree's at commit time (verified with `diff -q`).

## Design notes for T2/T3 consumers

- `scan::scan()` keeps the scaffolded `-> anyhow::Result<Manifest>` contract and
  **writes the manifest itself** to `paths::manifest_path()` (atomic: temp →
  fsync → rename → fsync(dir)). `push` therefore also refreshes it.
- `scan::scan_with(&roots, now_ms, minutes) -> ScanResult` is the injectable,
  testable core; `ScanResult { manifest, anomalies }`.
- `FileEntry.group` is the archive-unit key: Claude subagent files carry the
  **parent's** `session_id` *and* `group`, so a session set travels together.
- A file that breaks an assumption is **both** in the manifest **and** in
  `anomalies` — never silently filtered. Non-quiescent files likewise: reported,
  counted, `quiescent = false`, and it is T2's job to refuse to archive them.

## Commands run

```
git worktree add ../claude-transcript-archiver-t1-scan -b t1-scan
CARGO_TARGET_DIR=/Users/robertkarl/Code/claude-transcript-archiver/target cargo check --all-targets   # died in ml-kem
# in the /tmp copy with the 3 blockers shimmed:
CARGO_TARGET_DIR=$HOME/Code/tape-target cargo test --bin tape              # 7 passed
CARGO_TARGET_DIR=$HOME/Code/tape-target cargo clippy --bin tape            # 0 in my files
CARGO_TARGET_DIR=$HOME/Code/tape-target TAPE_HOME=/tmp/tape-t1-final cargo run --bin tape -- scan
CARGO_TARGET_DIR=$HOME/Code/tape-target TAPE_HOME=/tmp/tape-t1-final cargo run --bin tape -- scan --json
```

Sources stayed read-only: nothing under `~/.claude` / `~/.codex` / `~/.pi` was
modified after the run began. All writes went to `TAPE_HOME=/tmp/tape-*`. Scratch
dirs (`/tmp/tape-t1-scratch`, `/tmp/tape-t1-check`, `/tmp/tape-t1-*`,
`/tmp/tape-t1-retired`) were left in place — no `rm`, ever.
