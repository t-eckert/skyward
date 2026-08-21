#!/usr/bin/env bash
#
# Install a built skyward as a system service on a Debian-family Linux.
#
# Deliberately not a curl-into-bash installer and deliberately not idempotent
# magic: it does five specific things, prints each one, and refuses rather than
# guesses. Run it from the repository root after the binary is built.
#
#   cargo build --release --features usb
#   sudo ./deploy/install.sh
#
# What it does NOT do, on purpose:
#   - build anything (a Pi building Vite under memory pressure fails in ways
#     that are much easier to diagnose when you ran the build yourself)
#   - write /etc/skyward.env if one already exists (that file holds a receiver
#     position someone typed, and silently replacing it is unforgivable)
#   - start the service (run `systemctl start skyward` once you have read
#     `skyward doctor` output)

set -euo pipefail

BINARY=${BINARY:-target/release/skyward}
PREFIX=${PREFIX:-/usr/local/bin}
STATE=${STATE:-/var/lib/skyward}
ENV_FILE=${ENV_FILE:-/etc/skyward.env}
SERVICE_USER=${SERVICE_USER:-skyward}

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

die() { printf '\nerror: %s\n' "$1" >&2; exit 1; }
step() { printf '\n== %s\n' "$1"; }

[[ $EUID -eq 0 ]] || die "run this with sudo; it writes to /usr/local/bin, /etc and /var/lib"
[[ -f $BINARY ]] || die "$BINARY does not exist. Build it first:
    cargo build --release --features usb"

# Refuse a binary built for the wrong machine before installing it, rather than
# after, when the failure is a confusing exec format error from systemd.
if command -v file >/dev/null; then
	if ! file -b "$BINARY" | grep -qi "$(uname -m)"; then
		die "$BINARY does not look like a $(uname -m) binary:
    $(file -b "$BINARY")"
	fi
fi

step "Service user"
if id "$SERVICE_USER" >/dev/null 2>&1; then
	echo "  $SERVICE_USER already exists"
else
	# No login shell and no home: this account exists to own a directory and
	# hold a group membership.
	useradd --system --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER"
	echo "  created $SERVICE_USER"
fi
# plugdev is what the udev rule grants the dongle to.
usermod -aG plugdev "$SERVICE_USER"
echo "  $SERVICE_USER is in plugdev"

step "Binary"
install -m 0755 "$BINARY" "$PREFIX/skyward"
echo "  $PREFIX/skyward  ($("$PREFIX/skyward" --version))"

step "State directory"
install -d -o "$SERVICE_USER" -g "$SERVICE_USER" -m 0755 "$STATE"
echo "  $STATE"

step "USB access"
install -m 0644 "$here/udev/99-skyward-rtlsdr.rules" /etc/udev/rules.d/
install -m 0644 "$here/blacklist-rtl.conf" /etc/modprobe.d/
udevadm control --reload-rules
udevadm trigger
echo "  udev rule and DVB blacklist installed"
if lsmod | grep -q dvb_usb_rtl28xxu; then
	echo "  dvb_usb_rtl28xxu is loaded RIGHT NOW and holds the dongle."
	echo "  Unload it with:  sudo modprobe -r dvb_usb_rtl28xxu"
	echo "  (the blacklist only takes effect at the next boot)"
fi

step "Configuration"
if [[ -f $ENV_FILE ]]; then
	echo "  $ENV_FILE exists; leaving it alone"
else
	cat >"$ENV_FILE" <<-'ENVEOF'
		# skyward configuration. See docs/CONFIGURATION.md for every key.
		#
		# The receiver position can also be set from the web interface while the
		# receiver runs, which is usually easier than editing this file. Note the
		# precedence: a position set that way is persisted and OVERRIDES what is
		# here, and `skyward doctor` says so when the two disagree.
		SKYWARD_RECEIVER_LAT=
		SKYWARD_RECEIVER_LON=
		SKYWARD_RECEIVER_ALT_M=0

		# usb        -- this binary drives the dongle (needs the `usb` feature)
		# usb:1      -- the second dongle; `skyward devices` lists them
		# tcp:127.0.0.1:1234  -- via rtl_tcp, for a build without the feature
		SKYWARD_SOURCE=usb

		SKYWARD_SAMPLE_RATE_HZ=2400000
		# Must be a step the tuner actually offers. Maximum is often NOT optimal.
		SKYWARD_GAIN_DB=49.6

		SKYWARD_BIND=0.0.0.0:8080
		SKYWARD_DB_PATH=/var/lib/skyward/skyward.db
		SKYWARD_STATION_FILE=/var/lib/skyward/skyward-station.toml
		SKYWARD_RETENTION_HOURS=24

		# json so journald can parse fields rather than lines.
		SKYWARD_LOG_FORMAT=json
		RUST_LOG=info
	ENVEOF
	chmod 0644 "$ENV_FILE"
	echo "  wrote $ENV_FILE -- set the receiver position in it"
fi

step "Service"
install -m 0644 "$here/systemd/skyward.service" /etc/systemd/system/
systemctl daemon-reload
systemctl enable skyward >/dev/null
echo "  skyward.service installed and enabled (not started)"

cat <<'NEXT'

Installed. Before starting it:

  1. Put the receiver position in /etc/skyward.env, or leave it blank and set
     it from the web interface once the service is up.

  2. Check the dongle is visible and free:

         skyward devices

  3. Run the full self-check. Read every [warn] and [FAIL] line; each one ends
     in something to do:

         sudo -u skyward skyward doctor

  4. Start it, and watch the first minute:

         sudo systemctl start skyward
         journalctl -u skyward -f

Then open http://<this-machine>:8080/.
NEXT
