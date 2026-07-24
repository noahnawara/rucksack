#!/bin/sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
version=${RUCKSACK_VERSION:?RUCKSACK_VERSION is required}
application_identity=${RUCKSACK_APPLICATION_IDENTITY:?RUCKSACK_APPLICATION_IDENTITY is required}
installer_identity=${RUCKSACK_INSTALLER_IDENTITY:?RUCKSACK_INSTALLER_IDENTITY is required}
team_id=${RUCKSACK_TEAM_ID:?RUCKSACK_TEAM_ID is required}
distribution_dir="$repository_root/dist"
package_root=$(mktemp -d "${TMPDIR:-/tmp}/rucksack-package.XXXXXX")

cleanup() {
    /bin/rm -rf "$package_root"
}
trap cleanup EXIT HUP INT TERM

verify_code_identity() {
    binary=$1
    expected_identifier=$2
    signing_details=$(/usr/bin/codesign --display --verbose=4 "$binary" 2>&1)
    actual_identifier=$(printf '%s\n' "$signing_details" | awk -F= '/^Identifier=/ { print $2; exit }')
    actual_team_id=$(printf '%s\n' "$signing_details" | awk -F= '/^TeamIdentifier=/ { print $2; exit }')
    if [ "$actual_identifier" != "$expected_identifier" ]; then
        echo "Signed identifier for $binary is $actual_identifier; expected $expected_identifier" >&2
        exit 1
    fi
    if [ "$actual_team_id" != "$team_id" ]; then
        echo "Signed TeamIdentifier for $binary is $actual_team_id; expected $team_id" >&2
        exit 1
    fi
}

case "$version" in
    *[!0-9A-Za-z.+-]* | "")
        echo "Invalid package version: $version" >&2
        exit 1
        ;;
esac

if [ "${#team_id}" -ne 10 ]; then
    echo "RUCKSACK_TEAM_ID must contain exactly 10 uppercase letters or digits" >&2
    exit 1
fi
case "$team_id" in
    *[!A-Z0-9]*)
        echo "RUCKSACK_TEAM_ID must contain exactly 10 uppercase letters or digits" >&2
        exit 1
        ;;
esac

workspace_version=$(awk -F '"' '/^version = "/ { print $2; exit }' "$repository_root/Cargo.toml")
if [ "$version" != "$workspace_version" ]; then
    echo "Tag version $version does not match workspace version $workspace_version" >&2
    exit 1
fi

mkdir -p "$distribution_dir"

cd "$repository_root"
cargo build --workspace --release --locked --target aarch64-apple-darwin
cargo build --workspace --release --locked --target x86_64-apple-darwin

/usr/bin/lipo -create \
    target/aarch64-apple-darwin/release/rucksack \
    target/x86_64-apple-darwin/release/rucksack \
    -output "$distribution_dir/rucksack"
/usr/bin/lipo -create \
    target/aarch64-apple-darwin/release/rucksack-helper \
    target/x86_64-apple-darwin/release/rucksack-helper \
    -output "$distribution_dir/rucksack-helper"

/usr/bin/codesign \
    --force \
    --identifier io.rucksack.cli \
    --options runtime \
    --sign "$application_identity" \
    --timestamp \
    "$distribution_dir/rucksack"
/usr/bin/codesign \
    --force \
    --identifier io.rucksack.helper \
    --options runtime \
    --sign "$application_identity" \
    --timestamp \
    "$distribution_dir/rucksack-helper"

/usr/bin/codesign --verify --strict --verbose=2 "$distribution_dir/rucksack"
/usr/bin/codesign --verify --strict --verbose=2 "$distribution_dir/rucksack-helper"
verify_code_identity "$distribution_dir/rucksack" "io.rucksack.cli"
verify_code_identity "$distribution_dir/rucksack-helper" "io.rucksack.helper"

/usr/bin/install -d \
    "$package_root/usr/local/bin" \
    "$package_root/Library/PrivilegedHelperTools" \
    "$package_root/Library/LaunchDaemons"
/usr/bin/install -m 0755 "$distribution_dir/rucksack" "$package_root/usr/local/bin/rucksack"
/usr/bin/install -m 0755 \
    "$distribution_dir/rucksack-helper" \
    "$package_root/Library/PrivilegedHelperTools/io.rucksack.helper"
/usr/bin/install -m 0644 \
    assets/launchd/io.rucksack.helper.plist \
    "$package_root/Library/LaunchDaemons/io.rucksack.helper.plist"

package_path="$distribution_dir/rucksack-universal.pkg"
/usr/bin/pkgbuild \
    --root "$package_root" \
    --scripts "$repository_root/scripts/package/macos" \
    --identifier io.rucksack.pkg \
    --version "$version" \
    --install-location / \
    --ownership recommended \
    --sign "$installer_identity" \
    "$package_path"

/usr/sbin/pkgutil --check-signature "$package_path"
(
    cd "$distribution_dir"
    /usr/bin/shasum -a 256 "$(basename "$package_path")" > "$(basename "$package_path").sha256"
)
