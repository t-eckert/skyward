#!/usr/bin/env bash
#
# Fill in firstrun.sh.example and install it onto a freshly flashed boot
# partition, so a headless Pi comes up with a hostname, a user, an SSH key and
# a network without ever seeing a keyboard.
#
#   SKY_PASSWORD='...' SKY_WIFI_PSK='...' \
#     ./deploy/boot/provision.sh /Volumes/bootfs
#
# Run it on the machine with the card reader, right after `dd`. It is safe to
# re-run: it rewrites firstrun.sh and re-hooks cmdline.txt idempotently.
#
# Everything it writes lands on the FAT boot partition, which is readable by
# anyone who picks up the card. The Wi-Fi PSK and the password hash are
# therefore on that card in the clear until first boot consumes them — that is
# inherent to headless provisioning, not something this script chose, but it is
# worth knowing before you hand the card to someone.

set -euo pipefail

die() { printf '\nerror: %s\n' "$1" >&2; exit 1; }
step() { printf '\n== %s\n' "$1"; }

BOOT=${1:-}
[[ -n $BOOT ]] || die "usage: $0 <path-to-boot-partition>   (e.g. /Volumes/bootfs)"
[[ -d $BOOT ]] || die "$BOOT is not a directory"
[[ -f $BOOT/cmdline.txt ]] || die "$BOOT has no cmdline.txt; that is not a Raspberry Pi boot partition"

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
TEMPLATE=$here/firstrun.sh.example
[[ -f $TEMPLATE ]] || die "missing $TEMPLATE"

SKY_HOSTNAME=${SKY_HOSTNAME:-skyward}
SKY_USERNAME=${SKY_USERNAME:-thomas}
SKY_TIMEZONE=${SKY_TIMEZONE:-America/Toronto}
SKY_WIFI_COUNTRY=${SKY_WIFI_COUNTRY:-CA}
SKY_WIFI_SSID=${SKY_WIFI_SSID:-}
SKY_WIFI_PSK=${SKY_WIFI_PSK:-}
SKY_PUBKEY_FILE=${SKY_PUBKEY_FILE:-$HOME/.ssh/id_ed25519.pub}
# A stable UUID for the Wi-Fi keyfile. Regenerated on each run, which is fine:
# the connection is replaced wholesale, not merged.
SKY_WIFI_UUID=${SKY_WIFI_UUID:-$(uuidgen | tr '[:upper:]' '[:lower:]')}

[[ -f $SKY_PUBKEY_FILE ]] || die "no public key at $SKY_PUBKEY_FILE (set SKY_PUBKEY_FILE)"
PUBKEY=$(< "$SKY_PUBKEY_FILE")

# A password is still wanted even with key-only SSH: it is what lets you log in
# on a monitor and keyboard when the network is the thing that is broken.
if [[ -z ${SKY_PASSWORD:-} ]]; then
	read -rsp "Console password for $SKY_USERNAME: " SKY_PASSWORD; echo
	read -rsp "Again: " confirm; echo
	[[ $SKY_PASSWORD == "$confirm" ]] || die "passwords do not match"
fi
[[ -n $SKY_PASSWORD ]] || die "empty password"

if [[ -n $SKY_WIFI_SSID && -z $SKY_WIFI_PSK ]]; then
	read -rsp "Wi-Fi passphrase for $SKY_WIFI_SSID: " SKY_WIFI_PSK; echo
fi

# Linux's chpasswd -e wants a SHA-512 crypt hash ($6$...). Getting one is
# annoyingly platform-dependent: macOS ships LibreSSL, whose `passwd` has no
# -6, and Python's crypt module is gone in 3.13. Try each and say plainly
# which ones were tried if none works, rather than emitting a DES hash that
# would silently truncate the password to eight characters.
hash_password() {
	if openssl passwd -6 -salt "$(openssl rand -hex 8)" "$1" 2>/dev/null; then
		return 0
	fi
	if python3 -W ignore -c "import crypt,sys; sys.exit(0 if getattr(crypt,'METHOD_SHA512',None) in crypt.methods else 1)" 2>/dev/null; then
		SKY_PW="$1" python3 -W ignore -c \
			'import crypt,os; print(crypt.crypt(os.environ["SKY_PW"], crypt.mksalt(crypt.METHOD_SHA512)))'
		return 0
	fi
	return 1
}

HASH=$(hash_password "$SKY_PASSWORD") || die "no way to make a SHA-512 crypt hash here.
    Tried: openssl passwd -6 (LibreSSL does not have it), python3 crypt (removed in 3.13).
    Install GNU openssl (brew install openssl@3) and re-run."
[[ $HASH == \$6\$* ]] || die "produced a hash that is not SHA-512 crypt; refusing to use it"

step "Writing firstrun.sh"
# Substitute with awk rather than sed: the PSK, the hash and the key all
# contain characters (/, &, +, $) that sed's replacement syntax would eat.
export _H=$SKY_HOSTNAME _U=$SKY_USERNAME _P=$HASH _K=$PUBKEY \
       _S=$SKY_WIFI_SSID _W=$SKY_WIFI_PSK _C=$SKY_WIFI_COUNTRY _T=$SKY_TIMEZONE \
       _I=$SKY_WIFI_UUID
awk '
{
	gsub(/__HOSTNAME__/,      ENVIRON["_H"])
	gsub(/__USERNAME__/,      ENVIRON["_U"])
	gsub(/__PASSWORD_HASH__/, ENVIRON["_P"])
	gsub(/__SSH_PUBKEY__/,    ENVIRON["_K"])
	gsub(/__WIFI_SSID__/,     ENVIRON["_S"])
	gsub(/__WIFI_PSK__/,      ENVIRON["_W"])
	gsub(/__WIFI_COUNTRY__/,  ENVIRON["_C"])
	gsub(/__TIMEZONE__/,      ENVIRON["_T"])
	gsub(/__WIFI_UUID__/,     ENVIRON["_I"])
	print
}' "$TEMPLATE" >"$BOOT/firstrun.sh"
chmod 0755 "$BOOT/firstrun.sh" 2>/dev/null || true

grep -q '__[A-Z_]*__' "$BOOT/firstrun.sh" && die "unsubstituted placeholder left in firstrun.sh"
echo "  $BOOT/firstrun.sh  (hostname=$SKY_HOSTNAME user=$SKY_USERNAME wifi=${SKY_WIFI_SSID:-none})"

step "Hooking it into cmdline.txt"
# cmdline.txt must stay ONE line. A stray newline here is a kernel that boots
# without half its arguments, which presents as a Pi that does not come up at
# all and gives you nothing to read.
cp "$BOOT/cmdline.txt" "$BOOT/cmdline.txt.orig" 2>/dev/null || true
CMD=$(tr -d '\n' < "$BOOT/cmdline.txt")
CMD=$(printf '%s' "$CMD" | sed 's| systemd\.run=[^ ]*||g; s| systemd\.run_success_action=[^ ]*||g; s| systemd\.unit=[^ ]*||g')
printf '%s systemd.run=/boot/firmware/firstrun.sh systemd.run_success_action=reboot systemd.unit=kernel-command-line.target\n' "$CMD" >"$BOOT/cmdline.txt"
echo "  $(cat "$BOOT/cmdline.txt")"

[[ $(wc -l < "$BOOT/cmdline.txt") -eq 1 ]] || die "cmdline.txt is not a single line; restore cmdline.txt.orig"

cat <<NEXT

Provisioned. Eject the card, put it in the Pi, and power it up.

First boot runs firstrun.sh once and then reboots itself, so give it a
couple of minutes before expecting to reach it:

    ssh $SKY_USERNAME@$SKY_HOSTNAME.local

If that does not resolve, look for the lease on your router, or attach a
monitor -- /var/log/firstrun.log on the Pi records what happened.
NEXT
