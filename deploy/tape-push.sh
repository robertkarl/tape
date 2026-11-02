#!/bin/bash
# T6: the hourly-ish source-side push (decision D2: sources push, the archive
# never reaches out). Point the LaunchAgent at THIS script, not at `tape push`
# alone: push only ships blobs, so scan and store must run first.
#
# D9 (the ~/Code/ubdogs subset) is LIFTED as of 2026-11-02. Keep it that way: the
# CT takes the NEWEST manifest per host, so pushing with `--only <path>` would
# make a subset current again and `audit` would call everything else vanished.
set -uo pipefail

TAPE=/Users/robertkarl/Code/tape-target/release/tape
ARCHIVE=192.168.50.108
ATTEMPTS=3          # the first real launchd run failed with EHOSTUNREACH on a
RETRY_SLEEP=20      # Wi-Fi hiccup; a transient network error must not cost an hour

export HOME=/Users/robertkarl
export TAPE_HOME="$HOME/.local/share/tapes"
export PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin

# Never run two pushes at once: a tick that outlives itself would race the
# manifest and the blob store. macOS has no flock(1), so guard by process.
if [ "$(pgrep -f tape-push.sh | grep -vw "$$" | wc -l | tr -d ' ')" -gt 0 ]; then
  echo "another push is still running; skipping this tick"
  exit 0
fi

STATEDIR="$HOME/Library/Application Support/tapes"
mkdir -p "$STATEDIR"

# Never run two pushes at once: a tick that outlives itself would race the
# manifest and the blob store. macOS has no flock(1), so guard by process.
# A skipped tick is still RECORDED - an unrecorded skip makes the status page look
# current when it is stale, which is the one thing a dashboard must not do.
if [ "$(pgrep -f tape-push.sh | grep -vw "$$" | wc -l | tr -d ' ')" -gt 0 ]; then
  echo "another push is still running; skipping this tick"
  skiplog=$(mktemp); echo "SKIP_TICK" >"$skiplog"
  python3 - "$skiplog" "$STATEDIR" "$(date +%s)" 0 <<'PY'
import json,os,sys
runlog,statedir,started,rc=sys.argv[1],sys.argv[2],int(sys.argv[3]),sys.argv[4]
rec={"when":__import__('time').strftime('%Y-%m-%dT%H:%M:%SZ',__import__('time').gmtime(started)),
     "epoch":started,"duration_s":0,"ok":True,"kind":"skipped","attempts":0,
     "scan_files":None,"scan_non_quiescent":None,"stored":None,"already_present_store":None,
     "store_failures":None,"uploaded":None,"uploaded_mib":None,"to_send":None,
     "already_present_push":None,"manifest_remote":None,
     "error":"skipped: another push was already running"}
with open(os.path.join(statedir,'push-history.jsonl'),'a') as f: f.write(json.dumps(rec)+"\n")
tmp=os.path.join(statedir,'last-push.json.tmp')
open(tmp,'w').write(json.dumps(rec,indent=2)); os.replace(tmp,os.path.join(statedir,'last-push.json'))
print("status: skipped -> recorded")
PY
  exit 0
fi

started=$(date +%s)
runlog=$(mktemp)          # left in /tmp on purpose - this project never deletes

{
  echo "=== scan ==="
  "$TAPE" scan

  echo "=== store ==="
  "$TAPE" store           # store takes no --only: it reads the manifest scan wrote

  echo "=== push ==="
  rc=1
  for n in $(seq 1 "$ATTEMPTS"); do
    echo "--- attempt $n of $ATTEMPTS ---"
    if "$TAPE" push "$ARCHIVE" --user rk; then rc=0; break; fi
    [ "$n" -lt "$ATTEMPTS" ] && { echo "retrying in ${RETRY_SLEEP}s"; sleep "$RETRY_SLEEP"; }
  done
  echo "PUSH_RC=$rc"
} >"$runlog" 2>&1
push_rc=$(sed -n 's/^PUSH_RC=//p' "$runlog" | tail -1)
overall=$([ "${push_rc:-1}" = 0 ] && echo 0 || echo 1)

# Status surface: one JSON object appended per run, plus a self-contained page
# you can open any time. launchd's own stdout/stderr logs keep the raw output.
python3 - "$runlog" "$STATEDIR" "$started" "$push_rc" <<'PY'
import json, os, re, sys, time

runlog, statedir, started, push_rc = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
raw = open(runlog, errors='replace').read()

def num(pat):
    m = re.search(pat, raw, re.MULTILINE)
    return int(m.group(1)) if m else None

def last(pat):
    m = re.findall(pat, raw, re.MULTILINE)
    return m[-1] if m else None

# Which step failed, if any, and the error text launchd would have shown.
error = None
m = re.search(r'^Error: .*', raw, re.MULTILINE)
if m:
    tail = raw[m.start():m.start() + 400].replace('\n', ' | ')
    error = re.sub(r'\s+', ' ', tail)[:300]

rec = {
    "when": time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(started)),
    "epoch": started,
    "duration_s": int(time.time()) - started,
    "ok": push_rc == "0",
    "kind": "push",
    "attempts": len(re.findall(r'^--- attempt \d+', raw, re.MULTILINE)),
    "scan_files": num(r'^total: (\d+) files'),
    "scan_non_quiescent": num(r'^non-quiescent files: (\d+)'),
    "stored": num(r'^stored:\s+(\d+)'),
    "already_present_store": num(r'^already present:\s+(\d+)'),
    "store_failures": num(r'^failures:\s+(\d+)'),
    "uploaded": num(r'^uploaded:\s+(\d+)'),
    "uploaded_mib": num(r'^uploaded MiB:\s+(\d+)'),
    "to_send": num(r'^to send:\s+(\d+)'),
    "already_present_push": num(r'^already present:\s+(\d+)'),
    "manifest_remote": last(r'^manifest:\s+(\S+)'),
    "error": error,
}

hist = os.path.join(statedir, 'push-history.jsonl')
with open(hist, 'a') as f:
    f.write(json.dumps(rec) + "\n")

# last-push.json is written atomically (temp then rename), like everything else here.
tmp = os.path.join(statedir, 'last-push.json.tmp')
with open(tmp, 'w') as f:
    json.dump(rec, f, indent=2)
os.replace(tmp, os.path.join(statedir, 'last-push.json'))

runs = [json.loads(l) for l in open(hist, errors='replace').read().splitlines() if l.strip()][-12:]

# Staleness is the actual signal: a green row that is 6 hours old is a failure.
ok_runs = [r for r in runs if r['ok'] and r.get('kind') == 'push']
age_min = int((time.time() - (ok_runs[-1]['epoch'] if ok_runs else 0)) / 60)
stale = age_min > 45          # 3 missed ticks at the 15-minute interval

def row(r):
    kind = r.get('kind', 'push')
    cls = 'ok' if r['ok'] and kind != 'skipped' else 'bad'
    label = 'skipped' if kind == 'skipped' else ('ok' if r['ok'] else 'FAILED')
    detail = (f"stored {r['stored'] or 0} · uploaded {r['uploaded'] or 0} "
              f"({r['uploaded_mib'] or 0} MiB) · to send {r['to_send'] or 0}")
    if kind == 'skipped':
        detail = 'another push was running'
    err = f"<div class='err'>{r['error']}</div>" if r['error'] else ""
    return (f"<tr class='{cls}'><td>{r['when']}</td><td>{label}</td>"
            f"<td>{detail}</td><td>{r['duration_s']}s</td><td>{r['attempts']}</td></tr>{err}")

status = "OK" if rec['ok'] else "FAILED"
if rec.get('kind') == 'skipped':
    status = "SKIPPED (overlap)"
staleline = (f"<p class='bad'><b>no successful push in {age_min} minutes</b> — "
             f"check <code>push.err.log</code> and the archive host</p>" if stale else
             f"<p>last successful push: {age_min} min ago</p>")
banner = (f"<h1 class='{ 'ok' if rec['ok'] else 'bad' }'>last push {status}</h1>"
          f"<p>{rec['when']} · {rec['duration_s']}s · {rec['attempts']} attempt(s) · "
          f"{rec['scan_files'] or 0} files scanned, {rec['scan_non_quiescent'] or 0} not quiescent</p>"
          + staleline)
errblock = f"<pre class='err'>{rec['error']}</pre>" if rec['error'] else ""

html = f"""<!doctype html><meta charset=utf-8>
<meta http-equiv="refresh" content="900">
<title>tape · last push</title>
<style>
 body{{font:15px/1.5 -apple-system,system-ui,sans-serif;margin:2rem auto;max-width:56rem;color:#1a1a1a}}
 h1{{margin:0 0 .2rem}} h1.ok{{color:#157f3d}} h1.bad{{color:#b3261e}}
 p.bad{{color:#b3261e}}
 table{{border-collapse:collapse;width:100%;margin-top:1.5rem;font-size:14px}}
 th,td{{text-align:left;padding:.4rem .6rem;border-bottom:1px solid #ddd}}
 tr.bad td{{background:#fdecea}} tr.ok td{{}}
 .err{{color:#b3261e;white-space:pre-wrap;font-family:ui-monospace,monospace;font-size:13px}}
 a{{color:#0b5cad}}
</style>
{banner}
{errblock}
<p>Archive: <a href="http://192.168.50.108:8799/">tape serve · 192.168.50.108:8799</a>
 &nbsp;·&nbsp; raw log: <code>~/Library/Logs/tapes/push.out.log</code></p>
<table><tr><th>when (UTC)</th><th>result</th><th>what moved</th><th>took</th><th>tries</th></tr>
{''.join(row(r) for r in reversed(runs))}</table>
<p><small>Written by <code>deploy/tape-push.sh</code> on every run; this page is static and
self-refreshing every 15 minutes. History: <code>push-history.jsonl</code>.</small></p>
"""
with open(os.path.join(statedir, 'push-report.html'), 'w') as f:
    f.write(html)
print(f"status: {'ok' if rec['ok'] else 'FAILED'} -> {os.path.join(statedir,'last-push.json')}")
PY

cat "$runlog"      # keep the raw run in launchd's stdout log too
exit "$overall"
