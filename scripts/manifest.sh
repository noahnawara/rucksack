#!/bin/bash
# The checksum of every tracked file, written and checked in one place.
#
# SOURCE_MANIFEST.sha256 is what VALIDATION.md's claim rests on: it names the exact bytes that were
# compiled, tested, and reviewed. That is only worth something if it covers the whole tree, and for
# a long time it did not — the list of paths was read back out of the manifest itself, so it could
# only ever describe the files that were already in it. The website was added afterwards and never
# appeared. Neither did INSTALL.md. Four of the eight files `set-version.sh` rewrites were invisible
# to the manifest it regenerated immediately afterwards, including three that hand a `cargo install
# --tag` line to a person.
#
# So the list comes from `git ls-files` now. What is tracked is what is described; there is no
# second list to keep in step, and a file added to the repository cannot be missed.
#
# Usage:
#   scripts/manifest.sh --write     regenerate it
#   scripts/manifest.sh --check     verify it, and say what is wrong if it is
#
# `--check` is deliberately not a pull-request gate. Every change to any tracked file changes this
# file too, so gating on it would make concurrent branches conflict here and nowhere else, and the
# conflict would always be resolved by regenerating. It runs on release, which is the moment the
# manifest actually claims something.

set -euo pipefail

cd "$(dirname "$0")/.."

MANIFEST=SOURCE_MANIFEST.sha256

mode=${1:-}
case $mode in
    --write | --check) ;;
    *)
        echo "usage: scripts/manifest.sh --write | --check" >&2
        exit 1
        ;;
esac

# Every tracked file except the manifest, which cannot contain its own checksum.
#
# git quotes a path containing a newline or a quote, which would corrupt a line-oriented list. No
# such path exists here, and refusing is better than describing the tree wrongly.
tracked_files() {
    if git ls-files | grep -q '^"'; then
        echo "a tracked path needs quoting; this script cannot describe it safely" >&2
        git ls-files | grep '^"' >&2
        exit 1
    fi
    # Regular files only. git tracks symlinks as entries too, and `.agents/skills/` holds one that
    # points at a directory — `shasum` refuses a directory, which would fail the write rather than
    # skip the entry. `-f` resolves the link, so a symlink to a real file is still described by the
    # content it points at.
    git ls-files | grep -vxF "$MANIFEST" | while IFS= read -r path; do
        if [ -f "$path" ]; then
            printf '%s\n' "$path"
        fi
    done | LC_ALL=C sort
}

# Paths as the manifest lists them, with any leading `./` removed so they compare against
# `git ls-files`. Older manifests were written by `find` and carry the prefix; new ones do not.
listed_files() {
    grep -v '^#' "$MANIFEST" | sed -e 's/^[0-9a-f]*  //' -e 's|^\./||' | LC_ALL=C sort
}

if [ "$mode" = --write ]; then
    {
        echo "# Every tracked file in this repository, as of the last release."
        echo "#"
        echo "# Regenerate with scripts/manifest.sh --write, verify with scripts/manifest.sh --check."
        echo "# Between releases this describes the released tree and not your working copy, so a"
        echo "# difference here is expected rather than a fault. scripts/set-version.sh rewrites it,"
        echo "# and the release workflow refuses to publish a tree it disagrees with."
        tracked_files | tr '\n' '\0' | xargs -0 shasum -a 256
    } >"$MANIFEST"
    echo "wrote $MANIFEST: $(listed_files | wc -l | tr -d ' ') files"
    exit 0
fi

failed=0

# Coverage first. A manifest with correct hashes for two-thirds of the tree passes `shasum -c`
# without ever mentioning the third it says nothing about, which is exactly how this last rotted.
missing=$(comm -13 <(listed_files) <(tracked_files))
extra=$(comm -23 <(listed_files) <(tracked_files))
if [ -n "$missing" ]; then
    echo "tracked but not described by $MANIFEST:" >&2
    printf '%s\n' "$missing" | sed 's/^/  /' >&2
    failed=1
fi
if [ -n "$extra" ]; then
    echo "described by $MANIFEST but not tracked:" >&2
    printf '%s\n' "$extra" | sed 's/^/  /' >&2
    failed=1
fi

if ! shasum -a 256 -c "$MANIFEST" >/dev/null 2>&1; then
    echo "$MANIFEST does not match the tree it describes:" >&2
    shasum -a 256 -c "$MANIFEST" 2>&1 | grep -v ': OK$' | sed 's/^/  /' >&2
    failed=1
fi

if [ "$failed" -ne 0 ]; then
    echo >&2
    echo "run scripts/manifest.sh --write" >&2
    exit 1
fi

echo "$MANIFEST matches the tree: $(listed_files | wc -l | tr -d ' ') files"
