#!/usr/bin/env bash
#
# Harden a Raspberry Pi running skyward against random power loss and SD wear.
#
# The premise this exists for: the machine is on mains power with no UPS, in a
# place nobody looks at, and it *will* be unplugged mid-write. Nothing here
# makes that safe — only a read-only root would — but each step either removes
# a class of write from the card or makes the failure recover without a
# keyboard attached.
#
#   sudo ./deploy/pi-harden.sh
#
# Like install.sh: explicit steps, each one printed, refusing rather than
# guessing. The machine this runs on is one you cannot get a console on, so a
# step that would need a judgement call stops and says what it wanted.
#
# What it does NOT do, on purpose:
#   - touch /etc/skyward.env, the binary, or the unit (that is install.sh)
#   - make the root filesystem read-only — a much larger change with a real
#     cost at update time, and not what this deployment chose
#   - reboot. Several steps only take effect at the next boot and say so.

set -euo pipefail

die() { printf '\nerror: %s\n' "$1" >&2; exit 1; }
step() { printf '\n== %s\n' "$1"; }

[[ $EUID -eq 0 ]] || die "run this with sudo; it writes to /etc"
[[ -r /etc/os-release ]] || die "no /etc/os-release; this is meant for Raspberry Pi OS"

reboot_needed=0

step "Journal size"
# Deliberately persistent, not volatile. Volatile journald is the usual advice
# for SD cards and it is wrong here: it erases the evidence of the power cut on
# the power cut, and the only question you ever ask this machine afterwards is
# "what happened before it went down". skyward logs at startup and on warnings,
# not per message, so the write volume this costs is small.
install -d -m 0755 /etc/systemd/journald.conf.d
# 99-, not 10-. Raspberry Pi OS ships
# /usr/lib/systemd/journald.conf.d/40-rpi-volatile-storage.conf with
# Storage=volatile to spare the card, and drop-ins merge in lexical order
# across every directory systemd reads -- so a 10- file loses to it silently.
# Measured: Storage=persistent was set, and journald still wrote to
# /run/log/journal, i.e. the journal was thrown away on every boot.
rm -f /etc/systemd/journald.conf.d/10-skyward-size.conf
cat >/etc/systemd/journald.conf.d/99-skyward-journal.conf <<'CONF'
# Deliberately overriding the Raspberry Pi OS default of Storage=volatile.
#
# Volatile is the right call for a general-purpose Pi: it spares the card. It
# is the wrong call here, because it erases the evidence of the power cut ON
# the power cut, and "what happened before it went quiet" is the only question
# anyone ever asks an unattended receiver. skyward logs at startup and on
# warnings, not per message, so the write volume is small -- and the cap below
# bounds it regardless. Delete this file to go back to the stock behaviour.
[Journal]
Storage=persistent
SystemMaxUse=64M
SystemMaxFileSize=8M
SystemMaxFiles=8
CONF
systemctl restart systemd-journald
sleep 1
journalctl --flush >/dev/null 2>&1 || true
sleep 1
# Verify where journald is ACTUALLY writing rather than trusting the file we
# just wrote. This is the third setting in this script where a drop-in was
# silently outranked, so nothing here reports success from its own write.
journal_path=$(journalctl --header 2>/dev/null | awk '/File path/ {print $3; exit}')
case $journal_path in
	/var/log/journal/*)
		echo "  persistent, capped at 64M -- writing to $journal_path" ;;
	/run/log/journal/*)
		echo "  [FAIL] still writing to $journal_path (volatile)."
		echo "  Something is still forcing Storage=volatile. Check:"
		echo "      systemd-analyze cat-config systemd/journald.conf" ;;
	*)
		echo "  could not determine the journal path; check journalctl --header" ;;
esac

step "Scratch space in RAM"
# /tmp and /var/tmp are pure churn on a card that is read-mostly otherwise.
# systemd ships the unit; Debian just does not enable it.
# Debian moved this: it used to ship as a template in /usr/share/systemd and
# is now a static unit in /usr/lib/systemd/system. Look in both rather than
# silently skipping the step, which is what happened on trixie the first time.
tmp_unit=""
for candidate in /usr/lib/systemd/system/tmp.mount /lib/systemd/system/tmp.mount /usr/share/systemd/tmp.mount; do
	[[ -f $candidate ]] && { tmp_unit=$candidate; break; }
done
if [[ -n $tmp_unit ]]; then
	if systemctl is-enabled tmp.mount 2>/dev/null | grep -q '^enabled'; then
		echo "  tmp.mount already enabled"
	else
		systemctl enable tmp.mount >/dev/null 2>&1 && {
			echo "  tmp.mount enabled from $tmp_unit (takes effect at next boot)"
			reboot_needed=1
		} || echo "  could not enable tmp.mount ($tmp_unit); leaving /tmp on disk"
	fi
else
	echo "  no tmp.mount unit found anywhere; leaving /tmp on disk"
fi

step "Swap"
# Kept, not disabled -- a deliberate reversal of the obvious SD-card advice.
#
# On a 2 GB board, swap is the safety net that stops the OOM killer choosing
# the decoder, and `cargo build` with lto=thin and codegen-units=1 genuinely
# wants the headroom. Turning it off buys a little card life and risks losing
# the one process this machine exists to run.
#
# swappiness=1 is what makes that free: the kernel will page out only to avoid
# an OOM, so in normal operation nothing is written and the wear argument
# never applies. Insurance without the premium.
# The right swappiness depends entirely on WHERE the swap is, and on current
# Raspberry Pi OS it is not where the folklore assumes.
#
#   zram      -- compressed RAM. Costs no card writes at all, and on a 2 GB
#                board it is a genuine win: paging cold pages into zstd frees
#                real memory for the decoder. Wants a HIGH swappiness.
#   a swapfile -- writes to the SD card under exactly the memory pressure that
#                is already hurting the decode loop. Wants a LOW one.
#
# Measured on trixie: the active swap is /dev/zram0 and dphys-swapfile does not
# even exist as a unit, so the reflexive "swappiness=1 to save the card" is
# both unnecessary and actively counterproductive.
swap_is_zram=0
if [[ -r /proc/swaps ]] && grep -q '^/dev/zram' /proc/swaps; then
	# Only if EVERY active swap is zram; a mixed setup should follow the disk.
	if ! grep -v '^Filename' /proc/swaps | grep -qv '^/dev/zram'; then
		swap_is_zram=1
	fi
fi

install -d -m 0755 /etc/sysctl.d
if [[ $swap_is_zram -eq 1 ]]; then
	cat >/etc/sysctl.d/60-skyward-swap.conf <<'CONF'
# Swap here is zram: compressed RAM, no card writes. Paging to it is cheap, so
# let the kernel do it freely -- on a 2 GB board that is extra usable memory,
# not wear.
vm.swappiness=100
# Write dirty pages back sooner, in smaller batches. A power cut then loses
# less, and the SD card sees fewer large bursts.
vm.dirty_background_ratio=5
vm.dirty_ratio=10
CONF
else
	cat >/etc/sysctl.d/60-skyward-swap.conf <<'CONF'
# Swap here is disk-backed, so it writes to the SD card. Keep it as an OOM
# backstop only: 1 means "only when the alternative is killing something".
vm.swappiness=1
vm.dirty_background_ratio=5
vm.dirty_ratio=10
CONF
fi
sysctl -q --system 2>/dev/null || sysctl -q -p /etc/sysctl.d/60-skyward-swap.conf
if [[ $swap_is_zram -eq 1 ]]; then
	echo "  swap is zram (compressed RAM, no card writes); swappiness=$(cat /proc/sys/vm/swappiness)"
else
	echo "  swap is disk-backed; swappiness=$(cat /proc/sys/vm/swappiness)"
fi
# A leftover swapfile that nothing uses is pure wasted card.
if [[ -f /var/swap ]] && ! grep -q '/var/swap' /proc/swaps 2>/dev/null; then
	echo "  note: /var/swap exists ($(du -h /var/swap | cut -f1)) but is NOT active."
	echo "  Nothing uses it. Reclaim it with:  sudo rm /var/swap"
fi
if command -v swapon >/dev/null; then
	swapon --show --noheadings 2>/dev/null | grep -q . \
		&& echo "  active: $(swapon --show --noheadings | tr '\n' ' ')" \
		|| echo "  note: no swap is active; on a 2 GB board consider enabling it"
fi

step "Filesystem mount options"
# relatime (the kernel default) already limits atime writes to one per file per
# day, so noatime is a small win. It is included because it is free, and it is
# done defensively because a bad /etc/fstab on a headless box means a rescue
# trip with a card reader.
if grep -qE '^\s*[^#].*\s/\s.*noatime' /etc/fstab; then
	echo "  / already mounts noatime"
else
	cp /etc/fstab "/etc/fstab.bak.$(date +%Y%m%d%H%M%S)"
	awk '
		/^[[:space:]]*#/ { print; next }
		$2 == "/" && $4 !~ /noatime/ { $4 = $4 ",noatime"; print; next }
		{ print }
	' OFS='\t' /etc/fstab >/etc/fstab.new
	if findmnt --verify --fstab --tab-file /etc/fstab.new >/dev/null 2>&1; then
		mv /etc/fstab.new /etc/fstab
		echo "  added noatime to / (backup kept; takes effect at next boot)"
		reboot_needed=1
	else
		rm -f /etc/fstab.new
		echo "  REFUSING: the rewritten fstab did not verify. Left it alone."
		echo "  Add noatime to the / line by hand if you want it."
	fi
fi

step "Filesystem repair on boot"
# This is what actually recovers the root filesystem after an unclean cut, and
# it is a Raspberry Pi OS default. Check rather than assume — a card imaged
# from something else may not have it.
CMDLINE=/boot/firmware/cmdline.txt
[[ -f $CMDLINE ]] || CMDLINE=/boot/cmdline.txt
if [[ -f $CMDLINE ]] && grep -q 'fsck.repair=yes' "$CMDLINE"; then
	echo "  fsck.repair=yes is set in $CMDLINE"
else
	echo "  WARNING: fsck.repair=yes is NOT set in $CMDLINE."
	echo "  Without it an unclean shutdown can drop the Pi to a repair prompt"
	echo "  it will sit at forever, with no keyboard attached. Add it."
fi

step "Hardware watchdog"
# Restart=always in skyward.service handles a dead process. It cannot handle a
# wedged kernel, which is the other way an unattended receiver goes quiet.
if [[ -c /dev/watchdog ]]; then
	install -d -m 0755 /etc/systemd/system.conf.d
	# 99-, not 10-. Drop-ins are merged in lexical order across ALL the
	# directories systemd reads, and Raspberry Pi OS ships
	# /usr/lib/systemd/system.conf.d/40-rpi-enable-watchdog.conf with
	# RuntimeWatchdogSec=1m. A 10- file loses to it silently -- measured: the
	# effective value stayed 1min while this script cheerfully reported 15s.
	rm -f /etc/systemd/system.conf.d/10-skyward-watchdog.conf
	cat >/etc/systemd/system.conf.d/99-skyward-watchdog.conf <<'CONF'
# The BCM2711 watchdog. If systemd stops petting it the board resets, which is
# the only thing that recovers a wedged kernel on a machine nobody logs into.
# Raspberry Pi OS already enables this at 1m; 15s just shortens the outage.
# RebootWatchdogSec covers the other half: a reboot that hangs on shutdown.
[Manager]
RuntimeWatchdogSec=15s
RebootWatchdogSec=2min
CONF
	# Report what systemd will ACTUALLY use, not what we just wrote. Writing a
	# config file is not the same as the setting taking effect, and on this
	# machine that difference has now bitten twice.
	effective=$(systemctl show -p RuntimeWatchdogUSec --value)
	echo "  /dev/watchdog present; wrote 99-skyward-watchdog.conf (15s)"
	echo "  effective right now: ${effective:-unknown} (changes at next boot)"
	reboot_needed=1
else
	echo "  no /dev/watchdog. Add 'dtparam=watchdog=on' to /boot/firmware/config.txt"
	echo "  and re-run this script after a reboot."
fi

step "Unattended security updates"
if apt-get install -y unattended-upgrades >/dev/null 2>&1; then
	cat >/etc/apt/apt.conf.d/20auto-upgrades <<'CONF'
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
APT::Periodic::AutocleanInterval "7";
CONF
	echo "  unattended-upgrades installed and enabled (security origins only)"
else
	echo "  could not install unattended-upgrades; skipping"
fi

step "SSH password authentication"
# firstrun.sh deliberately leaves password auth ON so that a provisioning
# mistake in the key does not produce an unreachable box. This is where it
# gets turned off -- but only once there is demonstrably a key to get back in
# with. Refusing is the right answer here; a headless machine with neither a
# working key nor a password is one you recover with a card reader.
keyed=$(find /home -maxdepth 3 -name authorized_keys -size +0 2>/dev/null | head -5)
if [[ -z $keyed ]]; then
	echo "  REFUSING: no non-empty authorized_keys under /home."
	echo "  Leaving password authentication enabled -- disabling it now would"
	echo "  lock this machine out entirely."
else
	echo "  found keys:"
	while IFS= read -r k; do echo "    $k"; done <<<"$keyed"
	install -d -m 0755 /etc/ssh/sshd_config.d
	cat >/etc/ssh/sshd_config.d/20-skyward-hardening.conf <<'SSHD'
# Keys only. A receiver that is reachable over a tailnet should not also be
# guessable over the LAN.
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin no
SSHD
	if sshd -t 2>/dev/null; then
		systemctl reload ssh 2>/dev/null || systemctl reload sshd 2>/dev/null || true
		echo "  password authentication disabled; keys only"
		echo "  (Tailscale SSH, if enabled, is unaffected and is a second way in)"
	else
		rm -f /etc/ssh/sshd_config.d/20-skyward-hardening.conf
		echo "  REFUSING: sshd rejected the config; reverted and left it alone"
	fi
fi

step "Host power"
# Not a change -- a measurement, made here because everything above is about
# keeping this machine alive and an undervolted Pi is the failure that hides
# best. It does not crash; it drops its clock. A throttled Pi cannot consume
# 2.4 MS/s, so samples go missing, so the message rate falls, and every visible
# symptom points at the antenna. `skyward doctor` checks this too, but this
# script often runs before skyward is even built.
if command -v vcgencmd >/dev/null; then
	t=$(vcgencmd get_throttled | cut -d= -f2)
	tv=$((t))
	echo "  $(vcgencmd measure_temp), get_throttled=$t"
	(( tv & 0x1 ))     && echo "  [FAIL] UNDER-VOLTAGE RIGHT NOW"
	(( tv & 0x4 ))     && echo "  [FAIL] CURRENTLY THROTTLED"
	(( tv & 0x10000 )) && echo "  [warn] under-voltage has occurred since boot"
	(( tv & 0x40000 )) && echo "  [warn] throttling has occurred since boot"
	if (( tv & 0x50005 )); then
		cat <<'POWER'
  A supply rated for the board plus ~300 mA for the dongle, and a short thick
  cable. This is not a tuning problem and no antenna move will fix it -- it
  will present as poor reception for as long as it lasts.
POWER
	else
		echo "  power looks clean"
	fi
else
	echo "  no vcgencmd; skipping (not a Raspberry Pi?)"
fi

printf '\n'
if [[ $reboot_needed -eq 1 ]]; then
	cat <<'NEXT'
Done. Some steps only take effect at the next boot:

    sudo reboot

After it comes back, confirm the ones that matter:

    findmnt /tmp                 # want tmpfs
    swapon --show                # want no output
    journalctl --disk-usage      # want well under 64M
    systemctl show -p RuntimeWatchdogUSec
NEXT
else
	echo "Done. Nothing here needed a reboot."
fi
