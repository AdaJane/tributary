#!/usr/bin/env bash
# Fetch the SoundFonts named in soundfonts/manifest.toml and check them.
#
# The bytes are not in git — ~380 MB of PCM is a cost every clone would pay
# forever — so this is what stands between a release artifact and whatever
# a CDN happened to serve. Every file is checked against the manifest's
# sha256 before it is allowed to exist under its real name.
#
# Quiet on success: it prints progress to stderr while working and says
# nothing at all on stdout, so it composes in a build without a caller
# having to strip a banner.
#
# Modes:
#   (none)               fetch anything missing or wrong, verify all
#   --check              verify what is present; never download
#   --print-attribution  emit the THIRD-PARTY.md SoundFonts section
#   --print-default      emit the id of the default voice
#   --print-ids          emit one id per line
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly HERE
readonly MANIFEST="${MANIFEST:-$HERE/../soundfonts/manifest.toml}"
readonly DEST="${DEST:-$HERE/../soundfonts/dist}"

note() { printf '%s\n' "$*" >&2; }
die() { printf '%s\n' "$*" >&2; exit 1; }

# The manifest is TOML and this is bash, so the parse is deliberately
# narrow: one `[[soundfont]]` table per font, flat string/int values, no
# nesting. Reading it with python would drag a build-time interpreter
# dependency into the Pi image chroot for four fields.
#
# Emits one `key=value` block per font, blank-line separated.
entries() {
    awk '
        /^\[\[soundfont\]\]/ { if (n++) print ""; next }
        # Digits belong in the key class. Without them `sha256` and
        # `archive_sha256` do not match, every hash parses as empty, and
        # every file then compares unequal — which reads as "your download
        # is corrupt" rather than "this script cannot read its manifest".
        /^[a-z0-9_]+ *=/ {
            key = $1
            sub(/^[a-z0-9_]+ *= */, "")
            gsub(/^"|"$/, "")
            print key "=" $0
        }
    ' "$MANIFEST"
}

field() { # <block> <key>
    printf '%s\n' "$1" | sed -n "s/^$2=//p" | head -1
}

verify() { # <path> <sha256> -> 0 if it matches
    # An empty expected hash is a manifest or parser bug, never a verdict
    # about the file. Comparing against it would quietly call every
    # download corrupt, which is precisely the wrong thing to be told.
    [ -n "$2" ] || die "no sha256 in the manifest for $(basename "$1")"
    [ -f "$1" ] || return 1
    [ "$(sha256sum "$1" | cut -d' ' -f1)" = "$2" ]
}

fetch_one() { # <block>
    local block="$1" id sha url member archive_sha target tmp
    id="$(field "$block" id)"
    sha="$(field "$block" sha256)"
    url="$(field "$block" url)"
    member="$(field "$block" archive_member)"
    archive_sha="$(field "$block" archive_sha256)"
    target="$DEST/$id"

    if verify "$target" "$sha"; then
        note "ok       $id"
        return 0
    fi
    if [ "$MODE" = check ]; then
        [ -f "$target" ] && die "corrupt  $id — sha256 does not match the manifest"
        die "missing  $id — run scripts/fetch-soundfonts.sh"
    fi

    note "fetching $id"
    tmp="$(mktemp "$DEST/.$id.XXXXXX")"
    # shellcheck disable=SC2064 # $tmp is fixed at trap time on purpose.
    trap "rm -f '$tmp'" RETURN

    if [ -n "$member" ]; then
        # Some fonts have no stable direct download left and are fetched
        # from a source archive instead. The archive gets its own hash, so
        # a tampered tarball is caught before anything is unpacked from it.
        local arc="$DEST/.$id.archive"
        if ! verify "$arc" "$archive_sha"; then
            curl -sSL --retry 3 --fail -o "$arc" "$url" \
                || die "download failed: $url"
            verify "$arc" "$archive_sha" \
                || die "archive sha256 mismatch for $id — refusing to unpack it"
        fi
        tar -xzOf "$arc" "$member" > "$tmp" || die "no $member in the archive for $id"
        rm -f "$arc"
    else
        curl -sSL --retry 3 --fail -o "$tmp" "$url" || die "download failed: $url"
    fi

    verify "$tmp" "$sha" || die "sha256 mismatch for $id — refusing to install it"
    # Only now does it get its real name: a half-written or wrong file is
    # never visible under the name the daemon would load.
    mv "$tmp" "$target"
    note "ok       $id"
}

MODE=fetch
case "${1:-}" in
--check) MODE=check ;;
--print-ids)
    entries | while IFS= read -r line; do
        case "$line" in id=*) printf '%s\n' "${line#id=}" ;; esac
    done
    exit 0
    ;;
--print-default)
    # Paragraph mode with a newline field separator: one record per font,
    # one field per key. Without FS="\n" awk splits on spaces instead, and
    # every value with a space in it — every name, every author, every
    # description — silently loses everything after the first word.
    entries | awk -v RS='' -v FS='\n' '{
        id = ""; want = 0
        for (i = 1; i <= NF; i++) {
            if ($i ~ /^id=/) { id = substr($i, 4) }
            if ($i == "default=true") { want = 1 }
        }
        if (want) print id
    }'
    exit 0
    ;;
--print-attribution)
    printf '## SoundFonts\n\n'
    printf 'Installed to `/usr/share/tributary/soundfonts` and listed in the\n'
    printf 'console as built-in sounds. Fetched at build time and checked\n'
    printf 'against `soundfonts/manifest.toml`.\n\n'
    entries | awk -v RS='' -v FS='\n' '{
        delete f
        for (i = 1; i <= NF; i++) {
            k = $i; sub(/=.*/, "", k)
            v = $i; sub(/^[^=]*=/, "", v)
            f[k] = v
        }
        printf "- **%s** %s by %s — %s. <%s>\n", f["name"], f["version"], f["author"], f["license"], f["license_url"]
    }'
    exit 0
    ;;
'') ;;
*) die "usage: fetch-soundfonts.sh [--check|--print-attribution|--print-default|--print-ids]" ;;
esac

mkdir -p "$DEST"
[ -f "$MANIFEST" ] || die "no manifest at $MANIFEST"

while IFS= read -r -d '' block; do
    [ -n "$block" ] || continue
    fetch_one "$block"
done < <(entries | awk -v RS='' -v ORS='\0' '{ print }')
