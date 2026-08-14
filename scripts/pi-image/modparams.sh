#!/bin/sh
# Assert that every module parameter we set is one the SHIPPED module
# actually accepts.
#
# The kernel IGNORES a mistyped module parameter in silence — no error, no
# journal line, nothing. So a check that the file exists proves only that
# we wrote a file. This reads the parameter names out of the module's own
# `.modinfo` section, which works on a foreign-architecture rootfs because
# both paths below read ELF strings rather than executing anything.
set -eu

params() {
    module="$1"
    if command -v modinfo >/dev/null 2>&1 && modinfo -F parm "$module" 2>/dev/null | head -1 | grep -q .; then
        modinfo -F parm "$module" 2>/dev/null | sed 's/:.*//'
        return 0
    fi
    # The module is compressed on this base; `strings` over the decompressed
    # bytes finds the same `parm=<name>:<description>` entries modinfo reads.
    case "$module" in
    *.xz) xz -dc "$module" ;;
    *.zst) zstd -dc "$module" ;;
    *.gz) gzip -dc "$module" ;;
    *) cat "$module" ;;
    esac | strings | sed -n 's/^parm=\([A-Za-z0-9_]*\):.*/\1/p'
}

case "${1:-}" in
--print-params)
    params "${2:?usage: --print-params <module>}"
    ;;
--check)
    module="${2:?usage: --check <module> <conf>}"
    conf="${3:?usage: --check <module> <conf>}"
    # Nothing set is nothing to get wrong. Keeping the check wired against
    # an absent conf means the guard is already in place the day somebody
    # adds one.
    if [ ! -e "$conf" ]; then
        echo ok
        exit 0
    fi
    if [ ! -e "$module" ]; then
        # NOT ok: an unreadable module means the check proved nothing, and
        # a check that passes when it could not look is worse than none.
        echo "$(basename "$module"): module not found, cannot verify parameters"
        exit 0
    fi
    known="$(params "$module")"
    verdict=ok
    # `options <module> a=1 b=2` — every token after the module name.
    while read -r line; do
        case "$line" in
        options*) ;;
        *) continue ;;
        esac
        set -- $line
        shift 2
        for assignment in "$@"; do
            name="${assignment%%=*}"
            if ! echo "$known" | grep -qx "$name"; then
                verdict="$(basename "$module"): no such parameter: $name"
            fi
        done
    done < "$conf"
    echo "$verdict"
    ;;
*)
    echo "usage: modparams.sh --print-params <module> | --check <module> <conf>" >&2
    exit 2
    ;;
esac
