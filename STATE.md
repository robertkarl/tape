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
| **T5 renderer port + `serve`** | **done and live on the archive host** (`http://192.168.50.108:8799`). Report: `REPORT-T5.md` |
| **T6 launchd + `audit`** | **CT side done 2026-11-02:** `tape serve` is a **system unit**, `ingest` + `verify` are on timers, journald is fixed, `sudo` is installed, two reboots verified. Push plist + `tape-push.sh` written, **not installed**. `audit` is **not** coded — `src/main.rs:284` is still `bail!("not implemented (T6)")`. Everything in `deploy/README.md` |

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

1. **The firewall egress cut — the only T4 item left, and T5 is now built on
   the CT, so it can happen any time the human is present.** Not applied yet.
   Proposed CT-level rules (`/etc/pve/firewall/127.fw`): `policy_in DROP`,
   `policy_out DROP`, allow in from `192.168.50.0/24` for 22 and 8799 (8799 is
   `tape serve`, live now). Cutting it means every future `git pull` /
   `cargo build` on the CT needs the rule temporarily relaxed.
2. **Sizing — stays at 1024 MB for now.** The axum/tokio build landed without
   pressure; 512 MB is worth a try once the firewall work is done and the box
   is no longer being rebuilt.
3. **T6 launchd** will make the push hourly; nothing about it is blocked.
   `deploy/net.robertkarl.tapes.plist` + `deploy/tape-push.sh` are written and
   `plutil`-clean; installing them is a `launchctl bootstrap` away.
4. Cosmetic: `/var/lib/tapes/manifests/debra.json.tmp-{32367,32397,32688,32714}`
   are inert debris from the pre-fix push (they do not end in `.json`, so the
   loader ignores them). Delete only on request.

## T5 — done (2026-11-01)

Full write-up: **`REPORT-T5.md`**. The short version:

- `src/render/` is render_trace.py translated (template verbatim, badges,
  highlighter as a scanner with no regex crate), `src/web.rs` is axum with two
  pages. No `Cargo.toml` change.
- **Ran on the CT as `rk`** via `setsid nohup … tape serve`. That died with the
  CT's reboot (observed 2026-11-02: box up 3 h 58 m, `pgrep -x tape` → 0, port
  8799 dead). **Superseded** by `deploy/tape-serve.service`, a systemd user unit
  — see the T6 section below.
- M4 acceptance measured: Claude `7bd359d5` 35 turns / 17 subagent turns marked
  / 3 edit diffs; codex `019d68e5` 930 turns / 2503 coloured patch lines; pi
  `019e5ae1` 27 turns / 12 edit diffs; the CT's own `01a1899d` over the LAN.
  Largest session `e707f04a`: 8630 turns, 1.06 s to render, **45 MB page**.
- Guardrails proven by tests: `PRAGMA query_only` (an INSERT fails), filters are
  parameters (`'; DROP TABLE sessions; --` is just a value), `serve` refuses
  `0.0.0.0`, a flipped ciphertext byte is a `hash mismatch` error not a page.
- Known limits, none blocking: no FTS (so `q=` searches id/model/cwd only),
  45 MB pages for giant sessions want server-side turn paging, Codex sessions
  show no tokens/cost, `unmapped_lines` still has no column.

Two traps paid for, both worth remembering:

- **axum 0.8 routes are `{param}`, not `:param`** — it panics at startup.
- **`pkill -f "tape serve"` over ssh kills the ssh session itself** (the pattern
  matches the command line of the shell running it). Use `pkill -x tape`, or
  separate the kill and the start into different ssh calls.

## T6 — started (2026-11-02): `serve` is a service now, push is written, `audit` is not

> **This first attempt (a systemd *user* unit) was superseded the same day.**
> See "CT hardening" below: `serve` is a **system** unit now, the user unit was
> moved to `/home/rk/systemd-user-retired/`, and two reboots were verified.
> The findings in this subsection are kept because they explain why.

**`tape serve` on CT 127 is installed as a systemd user unit**
(`~/.config/systemd/user/tape-serve.service`, source: `deploy/tape-serve.service`).
Measured on the CT:

| check | result |
|---|---|
| `systemctl --user status` | `active` + `enabled`, Main PID 473, 4.6 MB |
| `ss -ltnp` | `192.168.50.108:8799` — LAN address, **not** `0.0.0.0` |
| `curl /` from debra | `200`, 6.8 KB, **42 ms** |
| `kill -TERM $(pgrep -x tape)` | `activating` → `active` in ~4 s, exactly one `tape`, `curl` 200 |

**Facts this uncovered, all three new:**

1. **CT 127 has no `sudo` at all** (Debian 13 standard template). So `rk` can
   only own *user* units. The one-time root step has to come from the proxmox
   host: `sudo -n pct exec 127 -- loginctl enable-linger rk`.
   *Fixed the same day — and the premise was wrong:* **root SSH into the CT
   works directly** (debra's key is in the CT's `authorized_keys` for root too,
   from `pct create --ssh-public-key`), so no proxmox hop was needed at all.
2. **`/var/lib/systemd/linger/` is empty** → no lingering → the unit starts on
   login, not yet at boot. That is the last mile, and it is human-only.
   *Fixed the same day, differently:* user units were the wrong shape for a
   headless box; a **system unit** removes the linger question entirely.
3. **The CT already trusts debra's default key.** `~/.ssh/authorized_keys` for
   `rk` holds exactly `id_ed25519`
   (`SHA256:7JOhh7Fki1cvrkNR9m7YW9mMUAqLIoKkNpaHJDcsvv4`), injected by
   `pct create --ssh-public-key` and wrapped in `# --- BEGIN PVE ---` markers.
   `tape push`'s default key works; no `--key` anywhere. `id_rsa` is *not* in
   that file — it only ever mattered for the loopback test.

Coupling to watch: the unit hardcodes `--addr 192.168.50.108` because `serve`
refuses `0.0.0.0` (D3), and the CT is `ip=dhcp`. A moved lease breaks the unit
and the firewall rule together; a static lease would remove that.

**The push side is written but not installed:** `deploy/net.robertkarl.tapes.plist`
(`plutil`-clean, hourly, logs to `~/Library/Logs/tapes/`) runs `deploy/tape-push.sh`,
which does `scan --only` → `store` → `push`. Two API facts that broke a first
draft: **`store` has no `--only`** (it reads the manifest, which carries the
subset), and **macOS has no `flock(1)`**, so the overlap guard is a `pgrep`.

**`audit` is not implemented.** `STATE.md` previously claimed "`audit` already
coded" — wrong; `src/main.rs:284` is `Cmd::Audit => anyhow::bail!("not
implemented (T6)")`. That is the remaining T6 work, and it must key on
`(session_id, hash)`, never path (see *Paths move under us*).

### Measured on 2026-11-02: this project's own transcripts went through the whole pipeline

`scan --only ~/Code/claude-transcript-archiver` → `store` → `push` → `ingest` → UI, all
from debra, no loopback fake:

| stage | result |
|---|---|
| scan | pi **4** files + claude **1** (703 KB) = 5 files, 5.46 MB; **1 non-quiescent** |
| store | 4 blobs, 5 MiB → 2 MiB (**2.68×**), 0 failures |
| push | dry-run 4/then uploaded **4**, verified by size 4, manifest `manifests/debra-20261102T055956Z.json` |
| CT `verify` | **14/14 ok**, 0 failures |
| CT `ingest` | ingested **4**, missing blob 0, parse failures 0, read/decrypt fails 0 |
| index now | **14 sessions / 1489 turns / 14 blobs**, 1 host |

Archived from this project: pi `01a193df` **517 turns** (45.0 M in / 353 k out tokens),
pi `01a193ef` 102, claude `1b70d6ff` 46, pi `01a19ac9` 6. The list page picked the new
rows up **without restarting `serve`**.

**This very session is `01a19aca`, and `scan` refuses it: `quiescent=false`.**
`QUIESCENCE_MINUTES = 10` (`src/scan.rs:29`) and the file is being written as we speak.
That is the guardrail doing its job — it lands on the next pass, which is what the
hourly job is for.

### The `linger` gap is worse than "doesn't boot": measured

`loginctl show-user rk` → **`Linger=no`**. With no lingering, `user@1000.service` is
torn down when the **last login session closes**, and it takes the user unit's process
with it. So the site exists only while someone is logged into the CT:

```
# last ssh session to the CT closed, polling 8799:
+6s 200   +12s 200   +18s 000   +24s 000   +30s 000   +36s 000   +42s 000
```

Symptoms that sent me hunting: `curl` failing at a constant **14.77 s** (`exit=28`,
couldn't connect) while the same URL returned 200 in 4 ms moments earlier; `ss -ltnp`
showing **no listener on 8799** while `systemctl --user is-active` said `active`;
`ExecMainStartTimestamp` marching forward (05:52:51 → 06:03:12 → 06:05:07) with
`NRestarts=0` — each login starts a fresh user manager, so it is not a restart loop.
Not OOM: `MemoryPeak=2.4 MB`, `Result=success`, no `ExecMainStatus`.

**One human command fixes it:** `sudo -n pct exec 127 -- loginctl enable-linger rk`.
Until then, `ssh -N rk@192.168.50.108` left open is the only thing keeping the port up.

**And the CT has no logs:** `systemd-journald` is `failed`, `journalctl` says *"No
journal files were found"*, `dmesg` is not readable by `rk`. So `journalctl --user -u
tape-serve.service` — the debugging instrument M5 assumes — does not exist. Needs
persistent journald (root) or the unit's own log file.

### A design finding that changes T6's `audit`

`--only` takes **exactly one path** (`Subset.cwd: PathBuf`, `manifest.only: Option<String>`),
and the CT's loader is **newest manifest per host wins**. Pushing the archiver subset
therefore *superseded* the ubdogs manifest — `manifests/` now holds
`debra-20261101T000303Z.json`, `…T000311Z.json`, `debra-20261102T055956Z.json`, and only
the last one is "current". Consequence: **`audit` diffing source-vs-archive from the
newest manifest alone would report all 10 ubdogs sessions as vanished.** Either `--only`
becomes multi-path, or `audit` must union the manifest history rather than trust the
newest. Decide before `audit` is written, not after.

### CT hardening (2026-11-02, agent-owned container over direct SSH)

Operating rule now: **the agent owns CT 127 and works on it over direct SSH**
(`root@192.168.50.108` key auth works; `rk` has `NOPASSWD` sudo); **the proxmox
host is checked with the human first.** No `pct exec` was needed for any of this.

Installed, from `deploy/` (source of truth stays in git):

| unit | what it does |
|---|---|
| `tape-serve.service` | system unit, `User=rk`, `Restart=always`, `MemoryMax=512M`, `ProtectSystem=strict` + `ReadWritePaths=/var/lib/tapes`, `StartLimitIntervalSec=0` |
| `tape-ingest.service` + `.timer` | `tape ingest` 2 min after boot, then every 15 min (`Persistent=true`) |
| `tape-verify.service` + `.timer` | `tape verify` (decrypt + re-hash every blob) weekly, Sunday ~04:30 |

What was done to the container, in order:

1. **`sudo` installed** + `/etc/sudoers.d/rk` = `NOPASSWD:ALL`, `visudo -cf` parsed OK.
   CT 127 had skipped the bootstrap in `~/homelab/docs/ADDING-A-NEW-SERVICE.md:49-53`;
   that omission is what made every root action need a proxmox round trip.
2. **Egress was already open** — `deb.debian.org` 200 in 0.13 s, and
   `/etc/pve/firewall/127.fw` **does not exist**, so no firewall change was
   needed for `apt-get`. The egress cut is still unwritten (proxmox side, human).
3. **journald fixed.** It failed at every boot with `status=243/CREDENTIALS`:
   Debian 13's `systemd-journald.service` has `ImportCredential=journal.*`, and an
   unprivileged LXC has no credentials dir. Drop-in
   `/etc/systemd/system/systemd-journald.service.d/no-creds.conf` with
   `[Service]` / `ImportCredential=` clears it; `Storage=persistent` set. Verified:
   `journalctl` now returns the units' own output. **The `failed` state after the
   fix was the start-limit (5 starts / 10 s), not the bug** — `reset-failed` first.
4. **User unit retired** (moved to `/home/rk/systemd-user-retired/`, not deleted),
   system units enabled.
5. **Boot race found and removed.** First reboot: `serve` failed once with
   `binding 192.168.50.108:8799 … Cannot assign requested address (os error 99)`
   — it started before DHCP assigned the address, and `Restart=always` healed it
   in 5 s. `network-online.target` does **not** cover DHCP inside an LXC guest.
   Added `ExecStartPre` waiting up to 60 s for an `inet` on `eth0`; second reboot
   is clean (`NRestarts=0`).

Acceptance, measured after the second reboot:

| check | result |
|---|---|
| `curl /` from debra, no session held | **200 at +10 s and every 10 s to +80 s** |
| `systemctl is-active tape-serve tape-ingest.timer tape-verify.timer systemd-journald` | all `active` |
| `NRestarts` after clean boot | **0** |
| ingest timer firing unprompted | fired at boot+2 min: `ingested: 0, already indexed: 4` |
| index across two reboots | **14 sessions / 1489 turns**, `blobs 14` |
| session page over the LAN | 200, 3.2 MB, 0.11 s |

**Still open, proxmox side (human):** the `127.fw` rules, ZFS snapshots of
`tank/tapes` (**zero snapshots exist**; the vzdump job covers
`102,103,104,105,106,111,114` — **127 is not in it**, and `tank/tapes` is
`backup=0`), a static DHCP lease, and the off-box copy of `identity.age`.

## Reusable fixtures (all under /tmp, none deleted)

- `TAPE_HOME=/tmp/tape-t3-full` — 848 blobs, verified, **indexed** (current parsers).
  This is the fixture the renderer is developed against (96 claude / 356 codex /
  291 pi, 14 Claude groups with subagents). `tape serve` was run against it on
  `127.0.0.1:8799`; that dev server has been stopped.
- Rendered pages kept for review: `/tmp/tape-t5-*.html`, `/tmp/tape-ct-*.html`,
  screenshots `/tmp/t5-*.png`.
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
