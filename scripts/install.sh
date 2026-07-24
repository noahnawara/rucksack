#!/bin/sh
set -eu

release_base_url=https://github.com/noahnawara/rucksack/releases/latest/download
package_name=rucksack-universal.pkg
installer_work_dir=$(mktemp -d "${TMPDIR:-/tmp}/rucksack-install.XXXXXX")
package_file="$installer_work_dir/$package_name"
checksum_file="$package_file.sha256"

cleanup() {
    /bin/rm -rf "$installer_work_dir"
}
trap cleanup EXIT HUP INT TERM

download_release_file() {
    source_url=$1
    destination_file=$2
    /usr/bin/curl \
        --fail \
        --location \
        --show-error \
        --proto '=https' \
        --tlsv1.2 \
        --retry 3 \
        --retry-all-errors \
        --output "$destination_file" \
        "$source_url"
}

download_release_file "$release_base_url/$package_name" "$package_file"
download_release_file "$release_base_url/$package_name.sha256" "$checksum_file"

expected_checksum=$(/usr/bin/awk 'NR == 1 { print $1 }' "$checksum_file")
case "$expected_checksum" in
    *[!0-9a-f]* | "")
        echo "The published Rucksack checksum is invalid." >&2
        exit 1
        ;;
esac
if [ "${#expected_checksum}" -ne 64 ]; then
    echo "The published Rucksack checksum must contain 64 hexadecimal characters." >&2
    exit 1
fi

actual_checksum=$(/usr/bin/shasum -a 256 "$package_file" | /usr/bin/awk '{ print $1 }')
if [ "$actual_checksum" != "$expected_checksum" ]; then
    echo "Rucksack package checksum verification failed." >&2
    exit 1
fi

/usr/sbin/pkgutil --check-signature "$package_file"
/usr/sbin/spctl --assess --type install --verbose=2 "$package_file"

echo
echo "Verified the signed and notarized Rucksack package."
echo "macOS will request administrator authentication once to install the power helper."
/usr/bin/sudo /usr/sbin/installer -pkg "$package_file" -target /

echo
echo "Rucksack is installed."
if [ -r /dev/tty ] && [ -w /dev/tty ]; then
    echo "Starting the guided agent and hotspot setup."
    /usr/local/bin/rucksack setup </dev/tty
else
    echo "Run 'rucksack setup' in an interactive terminal to finish onboarding."
fi
