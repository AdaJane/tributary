#!/bin/sh
# Pin every CPU policy to the performance governor.
#
# `ondemand` samples every 10-20 ms and a 256-frame period at 48 kHz is
# 5.33 ms, so a burst that starts at the idle clock misses periods while the
# governor ramps. Exits 0 with a journal line when there is no cpufreq at
# all: a future SoC without it must not fail the boot of a box that has no
# login account.
set -eu

SYSFS="${1:-/sys/devices/system/cpu/cpufreq}"
GOVERNOR=performance

policies() {
    # `ls` on a glob that matches nothing prints an error and exits 2, so
    # the glob is tested first.
    [ -d "$SYSFS" ] || return 0
    for policy in "$SYSFS"/policy*/scaling_governor; do
        [ -e "$policy" ] || continue
        echo "$policy"
    done
}

case "${1:-}" in
--print-policies)
    SYSFS="${2:?usage: --print-policies <cpufreq-root>}"
    policies
    exit 0
    ;;
--print-governor)
    echo "$GOVERNOR"
    exit 0
    ;;
esac

found=0
for policy in $(policies); do
    echo "$GOVERNOR" > "$policy" 2>/dev/null || true
    found=$((found + 1))
done
if [ "$found" -eq 0 ]; then
    logger -t tributary-performance "no cpufreq policies found; leaving CPU scaling alone"
else
    logger -t tributary-performance "set $found CPU policies to $GOVERNOR"
fi
