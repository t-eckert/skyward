#!/usr/bin/env bash
#
# Compare what is running on the Pi against what is in this repository.
#
# The deployment copies files into /etc and /usr/local/bin. They are *copies*:
# once installed they live independently, and editing one on the box to chase a
# problem is exactly the kind of thing that happens at 1am and is never undone.
# Nothing else notices, and from then on the repo quietly stops describing the
# machine.
#
#   ./deploy/verify-drift.sh [user@host]
#
# Exits non-zero if anything has drifted, so it can gate a deploy.
#
# Deliberately NOT checked: /etc/skyward.env. install.sh refuses to overwrite it
# precisely because it can hold a receiver position someone typed, so it is
# meant to differ from anything in git. It is reported as informational only.

set -uo pipefail

TARGET=${1:-thomas@skyward.local}
here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

# repo path : installed path
PAIRS=(
	"deploy/systemd/skyward.service:/etc/systemd/system/skyward.service"
	"deploy/systemd/skyward-tailscale-serve.service:/etc/systemd/system/skyward-tailscale-serve.service"
	"deploy/systemd/skyward-healthcheck.service:/etc/systemd/system/skyward-healthcheck.service"
	"deploy/systemd/skyward-healthcheck.timer:/etc/systemd/system/skyward-healthcheck.timer"
	"deploy/skyward-healthcheck.sh:/usr/local/bin/skyward-healthcheck.sh"
	"deploy/udev/99-skyward-rtlsdr.rules:/etc/udev/rules.d/99-skyward-rtlsdr.rules"
	"deploy/blacklist-rtl.conf:/etc/modprobe.d/blacklist-rtl.conf"
)

printf '\n== Comparing %s against this checkout\n\n' "$TARGET"

# One ssh round trip for every sum, rather than one per file.
paths=$(printf '%s\n' "${PAIRS[@]}" | cut -d: -f2)
remote=$(ssh -o BatchMode=yes -o ConnectTimeout=10 "$TARGET" \
	"for f in $(echo "$paths" | tr '\n' ' '); do
		if [ -e \"\$f\" ]; then printf '%s %s\n' \"\$(sudo md5sum \"\$f\" | cut -d' ' -f1)\" \"\$f\"; else printf 'MISSING %s\n' \"\$f\"; fi
	done" 2>/dev/null)

if [[ -z $remote ]]; then
	printf 'error: could not reach %s\n' "$TARGET" >&2
	exit 2
fi

drift=0
for pair in "${PAIRS[@]}"; do
	local_f=${pair%%:*}
	remote_f=${pair##*:}
	local_sum=$(md5sum "$here/$local_f" 2>/dev/null | cut -d' ' -f1)
	remote_sum=$(awk -v f="$remote_f" '$2 == f {print $1}' <<<"$remote")

	name=$(basename "$local_f")
	if [[ -z $remote_sum || $remote_sum == MISSING ]]; then
		printf '  %-42s NOT INSTALLED\n' "$name"
		drift=1
	elif [[ $remote_sum == "$local_sum" ]]; then
		printf '  %-42s ok\n' "$name"
	else
		printf '  %-42s DRIFTED   repo %s / pi %s\n' "$name" "${local_sum:0:8}" "${remote_sum:0:8}"
		drift=1
	fi
done

# The binary is not a file comparison -- it is built on the Pi, so compare what
# is installed against what was last built there.
installed=$(ssh -o BatchMode=yes "$TARGET" \
	'md5sum /usr/local/bin/skyward ~/skyward/target/release/skyward 2>/dev/null | cut -d" " -f1 | tr "\n" " "' 2>/dev/null)
set -- $installed
if [[ $# -eq 2 ]]; then
	if [[ $1 == "$2" ]]; then
		printf '  %-42s ok\n' "skyward (installed = last built)"
	else
		printf '  %-42s DRIFTED   a build was never installed\n' "skyward (installed vs built)"
		drift=1
	fi
fi

printf '\n  %-42s not compared (holds a hand-typed position)\n' "/etc/skyward.env"

if [[ $drift -eq 0 ]]; then
	printf '\nEverything installed matches this checkout.\n'
else
	printf '\nDrift found. `task deploy:config` reinstalls from the repo.\n'
fi
exit $drift
