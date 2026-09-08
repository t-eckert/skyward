#!/usr/bin/env bash
#
# Restart skyward when the decoder has gone quiet but the process has not.
#
# `Restart=always` in skyward.service covers a process that died. It cannot
# cover the failure this codebase calls its worst outcome: the unit stays
# `active`, the log stays silent, and no sample has arrived for hours.
#
# /healthz is built for exactly that — it reports freshness, not liveness, and
# returns 503 for `stalled` (no samples for 30 s). So a bare `curl -f` is the
# whole probe.
#
# Two rules keep this from doing more harm than good:
#
#   1. Restart only after THRESHOLD consecutive failures, not one. A single
#      missed probe during a checkpoint or an apt run is not a dead radio --
#      and neither is an unplugged dongle, which skyward recovers from on its
#      own without any help from here.
#
#   2. Never restart more than once per COOLDOWN. An unplugged antenna is
#      `stalled` forever, and a watchdog without a cap turns that into a
#      restart loop that buries the real cause under its own log spam.
#
# It says what it is about to do before it does it, so `journalctl -u
# skyward-healthcheck` reads as a history rather than a mystery.

set -euo pipefail

URL=${SKYWARD_HEALTH_URL:-http://127.0.0.1:8080/healthz}
UNIT=${SKYWARD_UNIT:-skyward.service}
# Ten minutes, not three. Measured on hardware: pulling the dongle makes
# /healthz report `stalled` within 30 s, but skyward recovers *in process* --
# it closes, reopens and replays the tuner settings, retrying with backoff
# capped at 30 s. A restart during that window would abort a recovery that was
# already working, reset every counter, and hide the mechanism that fixed it.
#
# So this must be slower than the thing it is supervising. Anything the
# reconnect loop can fix, it fixes inside a minute of the hardware coming back;
# ten consecutive failures means it genuinely cannot.
THRESHOLD=${SKYWARD_HEALTH_THRESHOLD:-10}
COOLDOWN=${SKYWARD_HEALTH_COOLDOWN:-900}

# /run is a tmpfs, so both counters reset at boot. That is what we want: a
# reboot is itself a remedy, and carrying a failure streak across one would
# make the first probe after a power cut trigger an immediate restart.
STATE=/run/skyward-healthcheck
mkdir -p "$STATE"
FAILS=$STATE/consecutive-failures
LAST=$STATE/last-restart

read_int() { [[ -r $1 ]] && cat "$1" 2>/dev/null || echo 0; }

if curl -fsS --max-time 5 "$URL" >/dev/null 2>&1; then
	if [[ $(read_int "$FAILS") -gt 0 ]]; then
		echo "healthy again: $URL"
	fi
	echo 0 >"$FAILS"
	exit 0
fi

fails=$(( $(read_int "$FAILS") + 1 ))
echo "$fails" >"$FAILS"

# A 503 carries a body explaining which way it is unhealthy; a connection
# refused does not. Surface whichever we got.
detail=$(curl -sS --max-time 5 "$URL" 2>&1 | head -c 400 || true)
echo "probe failed ($fails/$THRESHOLD): $URL -- ${detail:-no response}"

if [[ $fails -lt $THRESHOLD ]]; then
	exit 0
fi

now=$(date +%s)
since=$(( now - $(read_int "$LAST") ))
if [[ $(read_int "$LAST") -ne 0 && $since -lt $COOLDOWN ]]; then
	echo "would restart $UNIT, but the last restart was ${since}s ago (cooldown ${COOLDOWN}s)."
	echo "this has now failed $fails consecutive probes. A restart is not fixing it --"
	echo "check the antenna, the dongle, and 'journalctl -u ${UNIT%.service}'."
	exit 0
fi

echo "RESTARTING $UNIT after $fails consecutive failed probes."
date +%s >"$LAST"
echo 0 >"$FAILS"
systemctl restart "$UNIT"
