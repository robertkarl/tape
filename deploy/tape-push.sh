#!/bin/bash
# T6: the hourly source-side push (decision D2: sources push, the archive never
# reaches out). Point the LaunchAgent at THIS script, not at `tape push` alone:
# push only ships blobs, so scan and store must run first.
#
# D9 subset stays in the filter; it is written into the manifest, so store and
# push inherit it. On the CT this shows up as a subset scan, not a whole machine.
set -euo pipefail

TAPE=/Users/robertkarl/Code/tape-target/release/tape
ARCHIVE=192.168.50.108
# D9 (the ~/Code/ubdogs subset) is LIFTED as of 2026-11-02: the archive now holds
# the whole machine. Keep it that way. The CT takes the NEWEST manifest per host,
# so pushing with `--only <path>` would make a subset the newest manifest and
# every session outside it would look "vanished" to `audit`. If a subset is ever
# wanted again, `--only` has to become multi-path first - see STATE.md.

export HOME=/Users/robertkarl
export TAPE_HOME="$HOME/.local/share/tapes"
export PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin

# Never run two pushes at once: an hourly tick that outlives itself would race
# the manifest and the blob store. macOS has no flock(1), so guard by process.
if [ "$(pgrep -f tape-push.sh | grep -vw "$$" | wc -l | tr -d ' ')" -gt 0 ]; then
  echo "another push is still running; skipping this tick"
  exit 0
fi

# store takes no --only: it reads the manifest that `scan` just wrote.
"$TAPE" scan
"$TAPE" store
"$TAPE" push  "$ARCHIVE" --user rk
