#!/bin/sh
# Assert the MECHANISM behind limits.d, not just the file.
#
# `/etc/security/limits.d` only reaches a lingering systemd user manager if
# `pam_limits` is in the `systemd-user` PAM stack. A base bump that dropped
# it would make every rtprio line inert, silently, and nothing else in this
# image would notice. Assert the promise, not the inventory.
set -eu

check() {
    root="$1"
    found=""
    for candidate in "$root/etc/pam.d/systemd-user" "$root/usr/lib/pam.d/systemd-user"; do
        [ -f "$candidate" ] || continue
        found="$candidate"
        if grep -qE '^[^#]*pam_limits\.so' "$candidate"; then
            echo ok
            return 0
        fi
        # One level of @include, which is how Debian factors these.
        included="$(sed -n 's/^@include[[:space:]]\+//p' "$candidate")"
        for name in $included; do
            for dir in "$root/etc/pam.d" "$root/usr/lib/pam.d"; do
                [ -f "$dir/$name" ] || continue
                if grep -qE '^[^#]*pam_limits\.so' "$dir/$name"; then
                    echo ok
                    return 0
                fi
            done
        done
    done
    if [ -z "$found" ]; then
        echo "systemd-user: not found"
    else
        echo "systemd-user: no pam_limits"
    fi
}

case "${1:-}" in
--print-limits-check)
    check "${2:?usage: --print-limits-check <rootfs>}"
    ;;
*)
    echo "usage: pam-limits.sh --print-limits-check <rootfs>" >&2
    exit 2
    ;;
esac
