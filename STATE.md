# STATE — snapshot before compaction (2026-10-31, debra)

Read this together with `SPEC.md`, `PLAN.md`, `TASKS.md`, `prompts/T3.md`.
Nothing here is aspirational: every number was measured on this machine today.

## Where the code is

Single checkout `~/Code/claude-transcript-archiver`, branch **`main`**, clean
tree. Other worktrees and branch labels (`t1-scan`, `t2-store`) were removed
after merging; history is in the merge commits.

```
main HEAD 7455d8b docs: cut the escalation list …
        cf9055c integrate T1+T2: tape store reads the persisted manifest
        dab7a59 merge t2-store
        5800581 merge t1-scan
```

16 tests pass: 7 inside `src/scan.rs`/`src/manifest.rs`, 9 in `tests/store_tests.rs`.

## Done and measured

- `tape scan` — pi **292** files, claude **205** files / **99** groups (14 groups
  carry subagents; 106 subagent files, all with a sibling main file), codex
  **356** (348 + 8 archived), **853** total, **7** non-quiescent, 0 anomalies.
  Writes the manifest atomically to `paths::manifest_path()`; `--json` is clean
  on stdout, the summary goes to stdout via `scan::summarize`.
- `tape keygen` / `store` / `verify` — age X25519 over zstd-12, blobs
  `store/blobs/<blake3-of-plaintext>.tape`, write→fsync→rename, nothing can
  delete or rewrite a blob. **846 blobs, 2387 MiB → 888 MiB (2.69×),
  verify 846/846, 0 failures.** SIGKILL mid-store → 337 complete blobs, all
  readable; rerun filled the remaining 11; final 348/348 on the codex-only run.
- Not implemented: `push`, `ingest`, `serve`, `audit` (T4 / T3 / T5 / T6).

## Working fixture you can reuse instead of re-storing

`TAPE_HOME=/tmp/tape-merged` holds a **complete, verified 846-blob store** plus
a throwaway age identity (`identity.age`, mode 0400) and `pubkey.txt`.
T3's `ingest` can run against it immediately:

```bash
TAPE_HOME=/tmp/tape-merged ~/Code/tape-target/debug/tape verify   # 846/846
```

If /tmp was cleared, regenerate in ~3 minutes:
`TAPE_HOME=/tmp/tape-merged tape scan && tape store && tape verify`.

~3.3 GB of other agent scratch (`/tmp/tape-t2*`, `/tmp/tape-t1*`) is still there;
the human decides whether to delete it. No `rm` was used anywhere in this work.

## Proxmox state — ONE THING PENDING

- **`tank/tapes` dataset: created**, mountpoint `/tank/tapes`, empty, 33 TB
  available, `chown 100000:100000` (so an unprivileged CT can use it via
  `--mp0 /tank/tapes,mp=/var/lib/tapes,backup=0`).
- **CT 127: half-created and stuck.** `pct list` shows `127 stopped create`,
  config empty — my `pct create … && pct start …` chain was aborted mid-flight.
  Contains nothing. Pending human OK: `sudo pct destroy 127 --purge`.
- Learned syntax: for the `tank-vm-disks` zfspool storage, pass
  `--rootfs tank-vm-disks:vm-<ID>-disk-0,size=8G`. Passing the explicit
  `subvol-127-disk-0` name fails ("dataset does not exist").
- House conventions from existing CTs: unprivileged, `ip=dhcp` (all of them),
  `firewall=1` on net0, `features nesting=1`, `onboot` varies, memory 256–1024.
  `local-lvm` is **99.89% full** — never put a rootfs there.
- Agreed sizing: user said 512 MB–1 GB or less. Plan was **1024 MB + 2 GB swap +
  2 cores for the in-CT build**, then drop to 512 MB once built.
- T4 stays one deliberate block with the human watching: create → rk user + key
  → apt deps + rust → build in CT → ingest → **then** the firewall egress cut.

## Decisions already made (do not re-open)

1. **`age` pinned to 0.11.5** (X25519, no ML-KEM). age 0.12 pulls `ml-kem 0.2.x`
   (`kem =0.3.0-pre.0`) which cannot coexist with russh 0.65's `ml-kem 0.3.3`
   (`kem ^0.3` → 0.3.0); the build dies inside `ml-kem`. In transit is unaffected.
2. **Archive unit = `.jsonl` session files only.** Skipped on purpose: pi's
   21.9 MB `…jsonl.bak`, `sessions.sqlite`, 46 `.lock` files, and
   `~/.claude/history.jsonl`. Adding a root later is one line in
   `scan::source_roots()`.
3. **`tape serve` → port 8799**, LAN bind, no auth, no nginx, no certs.
4. The 11 empty pi cwd dirs are **audit data**, not a question.
5. The archive is **not a backup** — it is re-importable from the tools' own
   trees, single user, so a lost age identity is a nuisance, not a catastrophe.

## API surface T3 consumes (do not fork new types)

- `types::{Tool, FileEntry, Manifest, BlobRef}` — `FileEntry { tool, session_id,
  group, path, size, mtime_ms, blake3, quiescent }`. **`group` is the archive
  unit**: Claude subagent files carry the parent's `session_id` *and* `group`.
- `scan::{scan, scan_result, scan_with, summarize, source_roots, QUIESCENCE_MINUTES}`;
  `ScanResult { manifest, anomalies }`.
- `manifest::{read_manifest, write_manifest}` (atomic).
- `paths::{tape_home, state_db, manifest_path, store_dir, blobs_dir,
  identity_path, pubkey_path}` — `TAPE_HOME` overrides the root.
- `store::{load_identity, load_recipient, open_and_check, verify_blob_with,
  verify_all, blob_path, ZSTD_LEVEL, default_jobs}` — to read a blob back:
  derive the expected hash from the filename stem, `age::decrypt`, `zstd::decode_all`.
- `db::open` and `parse::{parse_pi, parse_claude, parse_codex}` are still
  `bail!("not implemented (T3)")` stubs. Schema is PLAN.md §4, verbatim.

## Parser traps already paid for

- **pi:** envelope `timestamp` is ISO-8601 but `message.timestamp` is **epoch
  milliseconds** — use the `> 1e11 ⇒ ms` heuristic. Filename ids drift (39 files
  end `_<id8>`, 253 end `_<full-uuid>`); line 1's `{"type":"session","id":…}` is
  authoritative.
- **claude:** first line is *not* a message record (`type: mode`,
  `permission-mode`, …); session id comes from the filename stem. Subagent dirs
  sit at exactly two depths: `<cwd>/<uuid>.jsonl` and
  `<cwd>/<uuid>/subagents/agent-*.jsonl`. Carry `isSidechain`, `uuid`/`parentUuid`.
- **codex:** all 356 files have `session_meta.payload.id` matching the filename
  tail. Only `response_item` feeds turns; `session_meta`, `turn_context`,
  `world_state`, `token_usage_record`, `compacted` become index columns.

## Build gotchas

- Shared `CARGO_TARGET_DIR=$HOME/Code/tape-target` → **one shared `debug/tape`**;
  with multiple worktrees it belongs to whichever branch built last. Build and
  run in one command; check `tape --help` before trusting a binary.
- An interrupted build can leave a **stale relinked binary** (CLI silently
  reverted to old stubs). Fix: `cargo clean -p tape`.
- Cold dep build ≈361 crates (russh, axum, bundled sqlcipher) ≈6 min; warm
  rebuilds are seconds.
- `git commit -m "…"` with backticks runs command substitution — quote messages
  with single quotes.

## Human-only items (only these)

1. Production age identity + `pass insert tapes/identity.age` (interactive gpg).
2. T4 with the human present (Proxmox sudo + CT firewall egress cut) — and right
   now also the `pct destroy 127 --purge` of the stuck partial CT.
