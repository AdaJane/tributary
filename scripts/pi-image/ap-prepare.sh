#!/usr/bin/env bash
# Prepare wlan0 for the Tributary access point, before NetworkManager reads
# its profiles. Two jobs the image cannot do at build time:
#
#   1. Stamp the per-device SSID. The Pi's serial is not knowable when the
#      image is built, so the shipped profile carries the bare prefix and
#      this fills in the suffix that keeps two units in one room apart.
#   2. Unblock the radio. Raspberry Pi OS ships
#      /etc/modprobe.d/rfkill_default.conf with `default_state=0`, so every
#      radio boots soft-blocked; NetworkManager would then find nothing to
#      bring up and the AP would never appear, with no error anywhere.
#
# Idempotent — safe on every boot, and re-running never double-suffixes.
set -euo pipefail

readonly AP_SSID_PREFIX="Tributary"
readonly PROFILE="${PROFILE:-/etc/NetworkManager/system-connections/tributary-ap.nmconnection}"
readonly CPUINFO="${CPUINFO:-/proc/cpuinfo}"
readonly MACHINE_ID="${MACHINE_ID:-/etc/machine-id}"

# The SSID this device should advertise. Last 4 hex digits of the Pi's
# serial, which survives a reflash — that is why it beats machine-id.
# machine-id is the fallback for hardware reporting no serial: stable for
# the life of the card, but regenerated if the card is rewritten.
ssid() {
    local id
    id="$(sed -n 's/^Serial[[:space:]]*:[[:space:]]*\([0-9a-fA-F]\{4,\}\)$/\1/p' "$CPUINFO" | tail -1)"
    if [ -z "$id" ] && [ -r "$MACHINE_ID" ]; then
        id="$(tr -cd '0-9a-fA-F' < "$MACHINE_ID")"
    fi
    [ -n "$id" ] || return 1
    printf '%s-%s' "$AP_SSID_PREFIX" "$(printf '%s' "${id: -4}" | tr '[:lower:]' '[:upper:]')"
}

# Exists so the derivation can be exercised against a fixture on a dev box
# (`CPUINFO=fixture ap-prepare.sh --print-ssid`) — no Pi, no root, no image.
if [ "${1:-}" = --print-ssid ]; then
    ssid
    exit
fi

# SSID first: a radio that fails to unblock should not also cost the name.
if new_ssid="$(ssid)"; then
    # ^ssid= matches whatever the last boot wrote, so this never re-suffixes.
    sed -i "s/^ssid=.*/ssid=$new_ssid/" "$PROFILE"
else
    # The AP coming up matters more than the suffix: keep the shipped
    # default SSID and make the reason loud in the journal.
    echo "ap-prepare: no serial in $CPUINFO and no $MACHINE_ID — keeping default SSID" >&2
fi

rfkill unblock wifi
