#!/bin/bash
# Move every pinned version reference in the tree to one new version.
#
# The version is not written down once. It is in the workspace manifest, in the two crate manifests
# that pin rucksack-core exactly, and in five places that hand a `cargo install --tag` line to a
# person or an agent — README, INSTALL.md, the website's install prompt, the JSON-LD on the site,
# and the test that asserts the two agree. Miss one and the docs pin a tag that is not what
# `--version` prints, which is worse than an obviously stale number: it installs.
#
# So the release edit is mechanical, and this does it mechanically. It rewrites, then proves the old
# version is gone and the new one is everywhere it was.
#
# Usage: scripts/set-version.sh 0.1.0-alpha.5
#
# Changes nothing else. It does not commit, tag, or push: what to release stays a decision, and this
# is only the typing.

set -euo pipefail

NEW=${1:-}
if [ -z "$NEW" ]; then
    echo "usage: scripts/set-version.sh <version>    e.g. 0.1.0-alpha.5" >&2
    exit 1
fi

# Anchored so a bare number cannot be mistaken for a version, and so the tag form vX and the plain
# form X are both derivable from one argument.
if ! printf '%s' "$NEW" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'; then
    echo "not a version: $NEW" >&2
    echo "expected MAJOR.MINOR.PATCH with an optional pre-release, e.g. 0.1.0-alpha.5" >&2
    exit 1
fi

cd "$(dirname "$0")/.."

OLD=$(grep -m1 '^version = ' Cargo.toml | sed 's/^version = "\(.*\)"$/\1/')
if [ -z "$OLD" ]; then
    echo "could not read the current version from Cargo.toml" >&2
    exit 1
fi
if [ "$OLD" = "$NEW" ]; then
    echo "already at $NEW" >&2
    exit 1
fi

# Named rather than globbed, so a stray copy of a file cannot quietly become part of a release.
FILES=(
    Cargo.toml
    crates/rucksack-cli/Cargo.toml
    crates/rucksack-helper/Cargo.toml
    README.md
    INSTALL.md
    site/index.html
    site/tests/site.spec.ts
    site/src/content/install-agent-prompt.txt
)

for file in "${FILES[@]}"; do
    if [ ! -f "$file" ]; then
        echo "missing: $file" >&2
        exit 1
    fi
done

for file in "${FILES[@]}"; do
    LC_ALL=C sed -i '' "s/${OLD//./\\.}/$NEW/g" "$file"
done

# The lockfile records the workspace members' own versions, so it goes stale the moment they move.
cargo update --workspace --offline >/dev/null 2>&1 || cargo update --workspace >/dev/null

# The manifest covers the files just edited, and a release that ships stale hashes has no manifest.
paths=$(awk '{print $2}' SOURCE_MANIFEST.sha256 | LC_ALL=C sort -u)
printf '%s\n' "$paths" | tr '\n' '\0' | xargs -0 shasum -a 256 > SOURCE_MANIFEST.sha256

# Prove it, rather than trusting the sed. A missed reference is the whole failure this prevents.
stale=$(grep -rn --fixed-strings "$OLD" "${FILES[@]}" Cargo.lock || true)
if [ -n "$stale" ]; then
    echo "still referencing $OLD after rewriting:" >&2
    echo "$stale" >&2
    exit 1
fi
if ! shasum -a 256 -c SOURCE_MANIFEST.sha256 >/dev/null 2>&1; then
    echo "SOURCE_MANIFEST.sha256 does not match the tree it just described" >&2
    exit 1
fi

echo "$OLD -> $NEW"
grep -rn --fixed-strings "$NEW" "${FILES[@]}" | sed 's/^/  /'
echo
echo "Nothing is committed. Review, then commit, tag v$NEW, and push the tag."
