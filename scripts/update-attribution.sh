#!/usr/bin/env bash
# Rewrite THIRD-PARTY.md's SoundFonts section from soundfonts/manifest.toml.
#
# The licence of every bundled sound has exactly one home. This regenerates
# the derived copy in the notices file, and `just attribution-check` fails
# the build when the two have drifted — the same treatment openapi.json
# gets, and for the same reason: a font added to the manifest and not
# attributed here would ship unattributed with nothing to notice.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly HERE
readonly NOTICES="${NOTICES:-$HERE/../THIRD-PARTY.md}"
readonly BEGIN='<!-- BEGIN soundfonts -->'
readonly END='<!-- END soundfonts -->'

grep -qF "$BEGIN" "$NOTICES" && grep -qF "$END" "$NOTICES" \
    || { echo "no soundfont markers in $NOTICES" >&2; exit 1; }

section="$("$HERE/fetch-soundfonts.sh" --print-attribution)"

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
{
    sed -n "1,/$BEGIN/p" "$NOTICES"
    printf '%s\n' "$section"
    sed -n "/$END/,\$p" "$NOTICES"
} > "$tmp"

# --check asks the only question that matters — does the file already say
# what the manifest says? Deliberately not `git diff`: that would answer
# "has this file been edited since the last commit", which is a different
# question and one that fails on every working tree mid-change.
if [ "${1:-}" = --check ]; then
    if ! diff -u "$NOTICES" "$tmp"; then
        echo "THIRD-PARTY.md does not match soundfonts/manifest.toml" >&2
        echo "run: just attribution" >&2
        exit 1
    fi
    exit 0
fi

cat "$tmp" > "$NOTICES"
