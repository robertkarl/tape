# deploy/ — how the archive runs on both machines

Two long-lived pieces, one per machine. Neither is installed by the code; they
are files here plus the commands below.

| piece | machine | state |
|---|---|---|
| `tape-serve.service` (systemd **user** unit) | archive CT 127, as `rk` | **installed, running, enabled** (2026-11-02) |
| `net.robertkarl.tapes.plist` + `tape-push.sh` | debra (launchd, hourly) | **written, not installed** |

## `tape serve` on CT 127 — installed

Why a *user* unit: **CT 127 has no `sudo` at all** (Debian 13 standard template),
so `rk` cannot write a system unit. User units are the only thing `rk` owns.

Installed on the CT as `rk`:

```sh
mkdir -p ~/.config/systemd/user
cp tape-serve.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now tape-serve.service
```

Measured 2026-11-02:

| check | result |
|---|---|
| `systemctl --user is-active` | `active`, `enabled`, Main PID 473, 4.6 MB |
| `ss -ltnp` | `192.168.50.108:8799` — the LAN address, not `0.0.0.0` |
| `curl /` from debra | `200`, 6.8 KB, 42 ms |
| `kill -TERM $(pgrep -x tape)` | `activating` → `active` in ~4 s, one `tape` process, `curl` 200 |

That `Restart=always` is the point: the previous arrangement (`setsid nohup`)
died on the CT's reboot and nothing brought it back.

### One step left, and it needs root

`/var/lib/systemd/linger/` is **empty**, so `rk` has no lingering: the unit
starts when `rk` logs in, **not** at boot. One-time fix, from the proxmox host
(`rk` there has `sudo -n`):

```sh
sudo -n pct exec 127 -- loginctl enable-linger rk
```

After that, `systemctl --user` runs for `rk` at boot with nobody logged in. This
is the only human-only item in this directory.

### Two things to know about the unit

- The bind address is **hardcoded**, because `tape serve` refuses `0.0.0.0`
  (decision D3: LAN plaintext, no auth). CT 127 is `ip=dhcp` — if the lease
  moves, the unit *and* the firewall rule both have to follow it. A static lease
  would remove that coupling.
- Hardening is on (`ProtectSystem=strict`, `ProtectHome=read-only`,
  `ReadWritePaths=/var/lib/tapes`, `NoNewPrivileges`). It matches what the code
  already promises (`PRAGMA query_only`). `ProtectHome=read-only` is safe only
  because the binary lives in `/home/rk/tape` and serve writes nothing there —
  if the build layout moves, revisit that line.

## The hourly push from debra — written, not installed

`tape-push.sh` is what the LaunchAgent runs, **not `tape push` alone**: `push`
only ships blobs, so `scan` and `store` have to run first. `store` takes no
`--only` — it reads the manifest, and `scan --only` already wrote the D9 subset
into it.

Install when ready (agent can do this on request):

```sh
mkdir -p ~/Library/Logs/tapes
cp deploy/net.robertkarl.tapes.plist ~/Library/LaunchAgents/
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/net.robertkarl.tapes.plist
launchctl kickstart -k gui/$(id -u)/net.robertkarl.tapes
tail -f ~/Library/Logs/tapes/push.out.log
```

Credential is already in place, verified 2026-11-02: the CT's
`~/.ssh/authorized_keys` for `rk` holds **debra's `id_ed25519`**
(`SHA256:7JOhh7Fki1cvrkNR9m7YW9mMUAqLIoKkNpaHJDcsvv4`), injected by
`pct create --ssh-public-key` and wrapped in `# --- BEGIN PVE ---` markers. That
is `tape push`'s default key, so the plist passes no `--key`. (`id_rsa` is not
in that file; it was only ever used for the loopback test.)

`tape-push.sh` guards against overlapping ticks with `pgrep`, because macOS has
no `flock(1)`.
