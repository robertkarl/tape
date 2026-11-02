# deploy/ — how the archive runs on both machines

Three long-lived pieces: one daemon and two timers on the archive container, one
scheduled batch job on the laptop. Nothing here is installed by the code; these
are the files plus the commands that install them.

| piece | machine | state |
|---|---|---|
| `tape-serve.service` (system unit) | archive CT 127 | **installed, enabled, two reboots verified** |
| `tape-ingest.service` + `.timer` | archive CT 127 | **installed** — boot+2 min, then every 15 min |
| `tape-verify.service` + `.timer` | archive CT 127 | **installed** — weekly, Sun ~04:30 |
| `net.robertkarl.tapes.plist` + `tape-push.sh` | debra (launchd) | **installed, bootstrapped, every 15 min** |

## Operating rule

The agent **owns CT 127** and works on it over direct SSH: `root@192.168.50.108`
takes debra's `id_ed25519` (installed by `pct create --ssh-public-key`), and `rk`
has `NOPASSWD` sudo. **The proxmox host is not touched without the human.** No
`pct exec` was needed for anything in this directory.

## Why a *system* unit, not a user unit

The first attempt was `~/.config/systemd/user/tape-serve.service`. It ran, and it
died: with `Linger=no`, `user@1000.service` is torn down when the **last login
session closes**, and it takes the daemon with it. Measured — after the last ssh
logout, 8799 answered `200` for ~12 s, then `000` forever. The tell was `curl`
failing at a constant **14.77 s** while `systemctl --user is-active` still said
`active` and `ss -ltnp` showed no listener.

A system unit is not tied to anybody's login, so the linger question disappears.
The retired user unit is at `/home/rk/systemd-user-retired/` on the CT (moved,
never deleted).

Install:

```sh
scp deploy/tape-*.service deploy/tape-*.timer root@192.168.50.108:/etc/systemd/system/
ssh root@192.168.50.108 'systemctl daemon-reload &&
  systemctl enable --now tape-serve.service tape-ingest.timer tape-verify.timer'
```

Measured after a clean reboot, with no session held open:

| check | result |
|---|---|
| `curl /` from debra | **200 at +10 s and every 10 s through +80 s** |
| `NRestarts` | **0** |
| ingest timer, unprompted | boot+2 min: `ingested: 0, already indexed: 4` |
| index across two reboots | 14 sessions / 1489 turns / 14 blobs |
| session page over the LAN | 200, 3.2 MB, 0.11 s |

## Two container defects found and fixed on the way

1. **No `sudo` in CT 127** — it skipped the bootstrap in
   `~/homelab/docs/ADDING-A-NEW-SERVICE.md:49-53`. Installed, plus
   `/etc/sudoers.d/rk` = `rk ALL=(ALL) NOPASSWD:ALL` (`visudo -cf` parsed OK).
2. **journald failed at every boot** with `status=243/CREDENTIALS`: Debian 13's
   unit has `ImportCredential=journal.*` and an unprivileged LXC has no
   credentials directory. Fixed by
   `/etc/systemd/system/systemd-journald.service.d/no-creds.conf` containing
   `[Service]` + `ImportCredential=`, plus `Storage=persistent`. Note the
   `failed` state you see right after the fix is the **start limit** (5 starts in
   10 s) — `systemctl reset-failed` before judging it.

Egress from the CT was already open (`deb.debian.org` → 200 in 0.13 s) because
`/etc/pve/firewall/127.fw` does not exist. If the egress cut is ever written,
`apt-get` on this box will need the rule relaxed for the duration.

## The boot race, and why the unit waits

First reboot after install:

```
tape[97]: Error: binding 192.168.50.108:8799
tape[97]:     Cannot assign requested address (os error 99)
```

`serve` started before DHCP had assigned the address. `Restart=always` healed it
in 5 s, but the unit now also waits explicitly:

```
ExecStartPre=/bin/sh -c 'for i in $(seq 1 60); do ip -4 -o addr show eth0 | grep -q "inet " && exit 0; sleep 1; done; exit 1'
```

`After=network-online.target` alone does **not** cover DHCP inside an LXC guest.
The bind address is hardcoded because `serve` refuses `0.0.0.0` (decision D3:
LAN plaintext, no auth), and the CT is `ip=dhcp` — a static lease would remove
the coupling between the unit, the firewall rule, and the DHCP server.

## The scheduled push from debra — installed

`~/Library/LaunchAgents/net.robertkarl.tapes.plist`, `RunAtLoad` +
`StartInterval 900` (15 min, matching `tape-ingest.timer`), logs in
`~/Library/Logs/tapes/`. Verify with:

```sh
launchctl print gui/$(id -u)/net.robertkarl.tapes | grep -E 'state|runs|last exit|run interval'
tail -f ~/Library/Logs/tapes/push.out.log
log show --predicate 'eventMessage CONTAINS "net.robertkarl.tapes"' --last 1d   # lifecycle
```

`tape-push.sh` is what the agent runs, **not `tape push` alone**: `push` only
ships blobs, so `scan` and `store` run first. It runs a **whole-machine** scan —
D9 (the `~/Code/ubdogs` subset) was lifted on 2026-11-02 and must stay lifted: the
CT takes the newest manifest per host, so a subset push would make a subset
current again and `audit` would report everything else as vanished. `store` takes
no `--only` at all — it reads the manifest `scan` wrote. macOS has no `flock(1)`,
so the overlap guard is a `pgrep`.

**It retries.** The first real run exited 1 on `No route to host (os error 65)`
while the CT was demonstrably up — a transient on debra. `push` is now attempted
3× with a 20 s sleep, and the 15-minute interval means one bad moment cannot cost
an hour.

### Status surface

Every run writes, under `~/Library/Application Support/tapes/`:

| file | what it is |
|---|---|
| `last-push.json` | the last run, temp-then-rename (atomic) |
| `push-history.jsonl` | appended forever |
| `push-report.html` | static dashboard, self-refreshes every 15 min — `open ~/Library/Application\\ Support/tapes/push-report.html` |

The report shows per-run counts, the error text of any failure, and **staleness**:
if no push has succeeded in 45 minutes it says so in red. A green row that is six
hours old is a failure, and that is the case a naive "last run: ok" hides. Skipped
ticks (overlap guard) are recorded too, so they can't masquerade as successes.

Console.app covers the other half: launchd's start/exit events are in the unified
log (`eventMessage CONTAINS "net.robertkarl.tapes"`), but a `StandardOutPath` job's
output never reaches the log — so Console answers "did it run", the report answers
"what did it do".

No root needed on debra, and nothing needs to stay alive: the archive is
append-only and idempotent, so a missed hour costs nothing. Credential is
already in place — the CT's `authorized_keys` holds debra's `id_ed25519`
(`SHA256:7JOhh7Fki1cvrkNR9m7YW9mMUAqLIoKkNpaHJDcsvv4`), which is `tape push`'s
default key, so the plist passes no `--key`.

## What is still the human's

- `/etc/pve/firewall/127.fw`: `policy_in DROP`, allow LAN 22 + 8799 (the egress cut).
- ZFS snapshots of `tank/tapes` — **zero snapshots exist**, the vzdump job covers
  `102,103,104,105,106,111,114` (not 127), and `tank/tapes` is `backup=0`.
- A static DHCP lease for the CT.
- An off-box copy of `identity.age` (`pass insert tapes/identity.age`, interactive gpg).
  Lose it and the archive is permanently unreadable.
