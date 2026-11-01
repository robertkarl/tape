# Transcript Archiver — Plan

Status: **plan for review**. No code written yet. Every number below was measured
on 2026-10-31 on `debra` (the Mac Studio), not assumed.

---

## 1. Goal / non-goals

**Goal.** A local-first, append-only archive of every agent transcript this
machine (and later: the laptop, dev boxes) produces, plus a trivial web UI to
read them. The archive must survive tool-level pruning, tool uninstall, and
vendor disappearance.

**Non-goals** (from SPEC.md, restated so nobody drifts): no OAuth, no TLS/cert
machinery, no split-DNS work, no password-over-HTTP, no cloud, no destructive
operations anywhere in the pipeline.

---

## 2. Measured ground truth (2026-10-31, debra)

| Source | Location on disk | Files | Size | Oldest session start |
|---|---|---|---|---|
| Claude Code | `~/.claude/projects/<enc-cwd>/<uuid>.jsonl` (+ `<uuid>/subagents/agent-*.jsonl`) | 205 jsonl (99 top-level sessions) | 827 MB | **2026-07-18** |
| Codex | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` (+ `~/.codex/archived_sessions/`, 8 files) | 348 | 1.4 GB | 2026-03-07 |
| pi | `~/.pi/agent/sessions/<enc-cwd>/<ISO-ts>_<id8>.jsonl` | 291 (84 dirs) | 142 MB | 2026-04-22 |
| **total** | | | **~2.4 GB** | |

**Retention evidence.** Claude's on-disk history starts 2026-07-18 and its
monthly session histogram is `2026-10: 85, 2026-09: 9, 2026-08: 2, 2026-07: 3`
— a 105-day window with a suspiciously thin tail — while pi reaches back 6
months and Codex 8. `cleanupPeriodDays` is unset in `~/.claude/settings.json`
(Claude Code's documented default is 30 days). So: **the pruning hypothesis is
well-supported but not proven on this host.** The archiver's `audit`
subcommand (§7) is how we prove or kill it, by diffing source-vs-archive
nightly and logging vanished session IDs.

**Infrastructure facts that constrain the design.**

- `debra` = this machine, <debra-lan-ip>, Tailscale `<debra-tailscale-ip>`, sshd
  **listening on :22** (Remote Login on) → the archive host can *pull* from here.
- `proxmox` = <proxmox-lan-ip>, reachable over LAN SSH as `rk` (key auth,
  passwordless sudo). ZFS pool `tank` has **33 TiB free**. Highest existing CT is
  126 → **CT 127 is free**.
- Tailscale on debra is the **GUI app only**; the `tailscale` CLI is not on PATH
  and the tailnet interface is currently **down** (no `100.x` address, ping to
  debra's own tailnet IP fails). So Phase 1 must work over plain LAN; Tailscale
  is the *laptop* transport, and needs the CLI enabled (`/Applications/Tailscale.app` ships `InstallTailscaleCLI.scpt`).
- Rust 1.97.1 / cargo on debra; only `aarch64-apple-darwin` target installed; no
  `zig` / `cargo-zigbuild` / musl cross toolchain. crates.io + static.crates.io
  reachable (200). Local cargo registry cache is 1.3 GB (`rusqlite`, `axum`,
  `zstd`, `rustls`, `clap`, `tokio`, `blake3` cached; `age`, `russh`, `askama` are not).
- `age` is **not** installed; `pass` and `gpg` are.
- House deployment procedure: `~/homelab/docs/ADDING-A-NEW-SERVICE.md`
  (unprivileged Debian 13 LXC, `rk` user + key, Pi-hole DNS, nginx CT 103
  at <nginx-ct-ip>). We will follow steps 1–2 and **skip** steps 3–4 (DNS +
  nginx/HTTPS) — those are exactly the parts SPEC.md puts out of scope.

---

## 2.1 Working subset (decided 2026-10-31): `~/Code/ubdogs` only

From here on, tape archives a **subset**: sessions whose recorded working
directory is `~/Code/ubdogs`. Everything else stays on the source machine,
unarchived, until the subset has been pushed, ingested and browsed.

Measured on debra at decision time:

| tool | how the subset is matched | files | size |
|---|---|---|---|
| pi | session dir `--Users-robertkarl-Code-ubdogs--` | 8 | 3.2 MB |
| codex | first line `session_meta.payload.cwd == /Users/robertkarl/Code/ubdogs` | 2 | — |
| claude | project dir `-Users-robertkarl-Code-ubdogs` | **0** | — |

**There is no Claude transcript on this machine for that project.** Three
Claude files *mention* ubdogs as a cwd partway through, but they belong to
sessions started in `~/Code` and `~/Code/regina`; they are not ubdogs sessions
and are not in the subset. Expect the subset to be pi + codex until Claude is
actually used there.

Matching rule, so "the subset" is a fact rather than a vibe: **the working
directory the tool recorded for the session**, compared exactly, encoded the
way that tool encodes it in its own directory name. For Codex, whose tree is
dated rather than slugged, read the first line and compare `payload.cwd`.
A substring match on the path slug is deliberately *not* used — it would sweep
in sibling checkouts (`ubdogs-api`) and worktrees.

Mechanism: `tape scan --only <path>` filters at scan time and the filter is
recorded in the manifest, so `store`, `push`, `ingest` and `audit` all inherit
it from the manifest instead of re-deciding it. Filtering happens only at scan;
nothing downstream ever deletes a blob because it fell out of the subset.

---

## 3. Architecture

One Rust binary, three roles, selected by subcommand. One cargo workspace.

```
  ┌─ debra (Mac Studio, always on) ─────────────┐      ┌─ proxmox ──────────────────┐
  │ ~/.claude ~/.codex ~/.pi                    │      │ CT 127 "tapes" (Debian 13, no  │
  │        │                                    │ SSH  │ egress after setup)            │
  │  tape collect  ──── manifest + bundles ────►│─────►│  tape ingest  →  store/        │
  │  (launchd, hourly)                          │ sftp │  tape serve   →  SQLite index  │
  └─────────────────────────────────────────────┘      │                    + HTML UI   │
                                                        └────────────────────────────────┘
  laptop / other hosts: tape collect --push, over Tailscale, when present
```

- `tape collect` — scan sources, find new/quiescent sessions, hash them,
  produce a manifest, push (or serve) encrypted bundles. Runs on every source
  host. On debra: a launchd LaunchAgent (hourly), mirroring the existing
  `~/Library/LaunchAgents/net.robertkarl.*.plist` pattern.
- `tape ingest` — on the archive host: verify, decrypt, decompress, upsert into
  SQLite. Idempotent, append-only.
- `tape serve` — on the archive host: HTTP listing + per-session HTML render.
- `tape audit` — diff "what the tools have now" vs "what we hold", log
  vanished sessions. This is the deletion-detection instrument.

Proposed name: **`tape`** (transcript archive). Naming is your call; it appears
in paths, the CT hostname, and the launchd label.

---

## 4. Storage: two layers, both on `tank`

**Layer 1 — durable archive (the benchmark/training artifact).**
Content-addressed, immutable bundles: `store/blobs/<blake3-of-file>.tape`
(zstd + age), plus per-host manifests. Nothing is ever rewritten or deleted;
a session that grows produces a new blob and the index records which is newest.
This layer alone is a complete backup and can rebuild Layer 2 from scratch.

**Layer 2 — query index (SQLite, disposable/rebuildable).**

```sql
CREATE TABLE hosts   (id TEXT PRIMARY KEY, name TEXT, first_seen INT);
CREATE TABLE sessions(id TEXT, host TEXT, tool TEXT,   -- tool: claude|codex|pi
                      cwd TEXT, git_branch TEXT, model TEXT,
                      started_ms INT, ended_ms INT, turn_count INT,
                      in_tokens INT, out_tokens INT, cost REAL,
                      blob TEXT,                      -- blob holding newest version
                      PRIMARY KEY(id, host));
CREATE TABLE messages(session_id TEXT, host TEXT, seq INT, role TEXT,
                      ts_ms INT, text_len INT, thinking_len INT,
                      tool_calls INT, is_sidechain INT);
CREATE TABLE tool_calls(session_id TEXT, host TEXT, seq INT, name TEXT,
                        verb TEXT,                    -- bash pipeline verbs / file basename
                        out_len INT, is_error INT);
CREATE TABLE blobs(hash TEXT PRIMARY KEY, path TEXT, bytes INT, stored_ms INT,
                   source_path TEXT, source_mtime_ms INT, tool TEXT);
CREATE TABLE vanished(id TEXT, host TEXT, tool TEXT, last_seen_ms INT);
```

Raw JSONL is **never** parsed-and-discarded: the blob holds the file verbatim.
The index exists only to list, filter, and render. Fidelity for future
training/benchmarking is Layer 1's job.

Budget: today ~2.4 GB raw per host; zstd on JSONL typically 5–8×. Trivial
against 33 TiB. Keep every superseded version — disk is not the constraint.

---

## 5. Encryption (the tenets, made concrete)

| Tenet | Implementation | Honest caveat |
|---|---|---|
| at rest | Layer-1 bundles encrypted with **`age`** (X25519) to the archive host's public key. Layer-2 SQLite opened through **`rusqlite`'s `bundled-sqlcipher`** feature, key in `/etc/tapes/` mode 0400. | SQLCipher adds a C build (bundled SQLite+OpenSSL) to the crate graph; it is the single heaviest dependency. |
| in transit | Every hop is SSH (LAN) or Tailscale (remote). The web UI is served **bound to the LAN bridge only**, no public interface, no nginx, no cert. | Browsing from a LAN browser is plaintext HTTP on your own switch. Deviation is explicit, not hidden. If you want zero plaintext, browse via Tailscale (needs the CLI enabled on debra) and bind the listener to the tailnet interface instead. **Decision D3.** |
| local first / only | No outbound calls from the archive CT after setup. | — |
| simplicity | One binary, one DB, no queue, no daemon besides `tape serve`, no cron framework, no nginx. | — |
| 100% compiled Rust | All three components in this repo are Rust. | The transport decision (§7) decides whether we also shell out to `/usr/bin/ssh`. |

**Key management (one keypair, boring).** `age-keygen` (via the `age` crate) on
the CT → private identity at `/etc/tapes/identity.age` (0400, root), public key
copied to each source host's `/etc/tapes/pubkey` or `~/.config/tapes/config.toml`.
Backup of the private key: `pass insert tapes/identity.age` on debra — losing it
means the archive is unrecoverable, so this is step 1 of M1, not an afterthought.

---

## 6. Cross-tool normalization

Canonical model (small on purpose):

```
Session { id, tool, host, cwd, git_branch, model, started, ended, turns[], usage }
Turn    { role, ts, thinking?, text?, tool_calls[] }
ToolCall{ id, name, args_json, result_text?, is_error }
```

Per-source parsers — verified against real files on this machine:

| Tool | Envelope | Notes for the parser |
|---|---|---|
| pi | `type: session` header (version 3, id, cwd, ts) then `type: message` with `message.content[]` = `thinking`/`text`/`toolCall`; `role: toolResult` lines carry `toolCallId` + `isError`. **`message.timestamp` is epoch-ms while the envelope `timestamp` is ISO-8601** — `render_trace.py` already got burned by this; parse both, use the magnitude heuristic (`>1e11` ⇒ ms). Also carry `usage`/`cost`, `stopReason`, `errorMessage`, and `model_change` / `thinking_level_change` / `session_info` lines. |
| Claude | `type: user` / `assistant` with `uuid`/`parentUuid` chain, `message.content[]` = `thinking`/`text`/`tool_use`/`tool_result`, plus `mode`, `permission-mode`, `attachment`, `file-history-snapshot`, `isSidechain`. Subagent transcripts live in a **directory next to the session file** (`<uuid>/subagents/agent-*.jsonl`) and must be archived as a set, not as loose files. `~/.claude/history.jsonl` (prompt history) is archived as a bonus artifact. |
| Codex | `session_meta` (id, cwd, cli_version, originator, base_instructions), `response_item`, `event_msg`, `turn_context`, `world_state`, `token_usage_record`, `compacted`. Only `response_item` feeds the turn view; everything else is preserved in the blob and summarized into the index. Also archive `~/.codex/archived_sessions/`. |

**Renderer.** Port `~/Code/regina/scripts/render_trace.py` to Rust as an
embedded asset: keep its CSS/JS and HTML structure **verbatim** (the density
modes, one-turn-at-a-time paging, resizable timeline rail, emoji-per-tool badges,
edit-as-diff rendering are all worth keeping and are pure string templating).
Reimplement only the parse → canonical-model step in Rust, and drive the same
template from the canonical model so all three tools render identically.
The Python original stays untouched; we copy its template into this repo with
attribution in a header comment.

---

## 7. Copy protocol (the "periodic copy from debra ↔ sqlite host")

Rules: **append-only, resumable, verified, never destructive.**

1. `collect` walks each source root, skipping files whose mtime is < 10 min old
   (quiescence window — never archive a session mid-write).
2. For each candidate: `blake3` hash. Compare against the local `~/.local/share/tapes/state.db`
   (a small SQLite of host-side seen hashes). Unchanged ⇒ skip.
3. Manifest = `[(source_path, tool, session_id, size, mtime, hash)]`.
4. Transfer: `tape push` opens SSH to the archive host (russh + russh-sftp,
   reusing `~/.ssh/id_ed25519`) and uploads only blobs whose hash the host
   reports missing. Server replies "have/haven't"; no overwrite is ever
   attempted, and a blob write is `write → fsync → rename`, so an interrupted
   run leaves no partial state.
5. `audit` re-lists sources and records session IDs that disappeared since last
   run into `vanished` — that is the pruning evidence, collected automatically.
6. Failure posture: launchd reruns hourly; every step is idempotent; nothing
   ever deletes or truncates a source file. If a transfer fails the source is
   untouched by construction.

**Direction.** debra has sshd on :22, so either shape works. Recommendation:
`collect --push` from every source host (uniform code path for debra and the
laptop; the archive host needs no inbound SSH to anyone and, after setup, no
egress at all). A `tape pull --from debra` on the CT is a 20-line convenience
around the same machinery if you prefer the server to be the scheduler. **Decision D2.**

**As built (T4, 2026-10-31).** One SSH connection, one SFTP channel, no exec'd
`ssh`, no server-side helper. The diff is derived from the archive host's own
blob directory: blob *names* are content hashes, so `readdir` is the
"what do you already have" reply. Uploads go to
`<hash>.tape.tmp-<pid>` and are renamed into place, then re-statted and
size-checked; the temp name deliberately does not match `*.tape`, so a crashed
push leaves inert debris that `ingest` never reads. The manifest is pushed to
`TAPE_HOME/manifests/<host>.json`, which is what lets several source machines
share one index and what carries the D9 subset filter to the archive host.
Host keys are trust-on-first-use in tape's own `TAPE_HOME/known_hosts` (never
`~/.ssh/known_hosts`), mode 0600, and a *changed* key is a hard refusal.

Two API traps worth remembering: `russh_sftp::SftpSession::write` opens with
`WRITE` but **not** `CREATE`, so it cannot create a file — the one thing push
needs; and `russh` maps a `None` hash algorithm for RSA keys to legacy SHA-1
`ssh-rsa`, which OpenSSH has disabled, so RSA keys must ask for
`HashAlg::Sha256` explicitly.

---

## 8. Deployment (CT 127 on proxmox)

Follow `~/homelab/docs/ADDING-A-NEW-SERVICE.md` steps 1–2, skip 3–4:

```
sudo pct create 127 local:vztmpl/debian-13-standard_13.1-2_amd64.tar.zst \
  --hostname tapes --memory 1024 --cores 2 --swap 2048 \
  --net0 name=eth0,bridge=vmbr0,ip=dhcp,firewall=1 \
  --rootfs tank-vm-disks:8 --unprivileged 1 --onboot 1 \
  --mp0 /tank/tapes,mp=/var/lib/tapes,backup=0 \
  --ssh-public-key /tmp/debra-ed25519.pub
```

That is what was actually run (2026-10-31), with three traps recorded:
`local-lvm` is 99.89% full so the rootfs goes on `tank-vm-disks`; the rootfs
argument is `storage:SIZE` to *create* a volume (`storage:volname` addresses an
existing one and fails); and `pct create` otherwise blocks on a root-password
prompt, so feed it `</dev/null` or supply `--ssh-public-key`, which takes a
**file path**, not key text.

- Data on `tank`: create `tank/tapes` (or a dataset mount) and bind-mount into
  the CT at `/var/lib/tapes` so the archive lives on the RAIDZ2 pool, not the
  8 GB rootfs.
- **Egress cut after setup** (SPEC): Proxmox firewall on the CT — default-deny
  out, allow only LAN subnets (and the tailnet if used). Install `apt` packages
  and build deps *before* the cut; afterwards the CT is an island.
- **No** Pi-hole entry, **no** nginx/HTTPS, **no** DNS work.
- `tape serve` binds the LAN address only, port 8799 (locked). No public interface.

**Build/distribution gap to resolve (Decision D1).** The CT is `amd64`; debra is
`aarch64-apple-darwin`, and no Linux cross toolchain is installed. Options:
(a) build inside the CT (`cargo` + `build-essential` + `pkg-config`, needs
internet at build time — fine, before the egress cut); (b) cross-compile a
static `x86_64-unknown-linux-musl` binary on debra (needs `cargo-zigbuild` or
`brew install zig` — a dependency install, which per AGENTS.md needs your OK).
Recommendation: **(a)** for M1, revisit (b) only if we want source-free deploys.

---

## 9. Milestones

| # | Deliverable | Acceptance check |
|---|---|---|
| **M0** | `tape collect` local-only: scan all 3 sources, print manifest + hashes, write nothing outside `~/.local/share/tapes/`. | Counts match §2 (99 Claude sessions / 348 Codex / 291 pi). Dry-run output reviewed by you. |
| **M1** | age keypair generated, key backed up to `pass`; blobs written locally, zstd+age; `tape verify` round-trips every blob. | `tape verify` reports 0 failures over all ~2.4 GB. |
| **M2** | CT 127 up, egress cut, `push` over SSH, `ingest` into SQLCipher SQLite. | `tape audit` on debra vs CT agree on session IDs; kill a transfer mid-way, rerun, verify no partial blobs. |
| **M3** | Canonical parsers for pi + Claude + Codex → index tables. | Per-tool row counts match blob counts; spot-check 5 sessions per tool by hand. |
| **M4** | `tape serve`: list (filter by tool/host/date/cwd), session page from the ported renderer. | Open a pi, a Claude, and a Codex session in a LAN browser; Claude page renders subagents; edit calls render as diffs. |
| **M5** | launchd hourly job on debra + `audit` logging; laptop client over Tailscale. | 7 days of logs; `vanished` table either proves or kills the pruning hypothesis. |

Each milestone is independently useful; M2 alone already ends the data-loss risk.

---

## 10. Risks

| Risk | Mitigation |
|---|---|
| Transcript contains secrets (tokens in `bash` outputs, `.env` reads). | Encrypted at rest (age + SQLCipher), no public listener, no egress from CT. If you later want redaction, it happens at render time only — never by mutating the blob, which must stay faithful. |
| SQLCipher build friction on Debian 13. | `bundled-sqlcipher` compiles SQLite+crypto in-tree; fall back to plaintext SQLite + age-only Layer 1 (**Decision D4**). |
| russh/sftp surprises (key formats, macOS OpenSSH keys). | Fallback: exec `/usr/bin/ssh` as a transport pipe. Still no Python, but it is an external binary — flag in the tenets ledger. **Decision D5.** |
| Session files grow forever; huge blobs (Codex sessions are individually large; 1.4 GB total). | Quiescence window + content-addressed supersede; zstd; keep all versions (33 TiB free). |
| Claude's subagent directories get orphaned. | Archive unit = session *set* (main jsonl + `<uuid>/subagents/*`), keyed by session id. |
| Tailscale is down on debra and its CLI isn't installed. | Phase 1 is LAN-only; Tailscale work is M5 and explicit. |
| Any accidental deletion during development. | No `rm` in any code path or in my own shell work — move to `/tmp`. Archive host opens source trees read-only. |

---

## 11. Decisions I need from you before writing code

1. **D1 — where to build the Linux binary:** inside CT 127 (needs internet at
   build time, simplest.
2. **D2 — scheduler location:** source pushes (debra launchd → CT)
3. **D3 — web UI exposure:** LAN-plaintext HTTP
4. **D4 — index at rest:** SQLCipher
5. **D5 — transport:** in-process `russh`/`russh-sftp`
6. **Name:** `tape`
7. **CT ID 127 on proxmox with `tank/tapes` -- store this on the spinning disks (redundant)
8. **Scope of M0–M2:** debra only.
