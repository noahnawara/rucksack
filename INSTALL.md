# install rucksack

Use only [noahnawara/rucksack](https://github.com/noahnawara/rucksack). Pin one exact
tag or commit before changing the Mac, and read this file plus [SECURITY.md](SECURITY.md)
at that same ref.

rucksack supports macOS 14 or newer and Codex, Claude Code, and Cursor. There is no
Homebrew, npm, or `cargo install` distribution.

## choose the install

Use a signed release only when the selected stable GitHub release contains both:

- `rucksack-universal.pkg`
- `rucksack-universal.pkg.sha256`

Inspect `scripts/install.sh` at the selected tag. The script downloads from
`releases/latest`, so immediately before running it, confirm that the selected tag is still
the latest stable release. Stop if the tag, package, or checksum does not match.

Run the inspected script from that tag:

```sh
/bin/sh scripts/install.sh
```

Let its HTTPS retry, SHA-256, package-signature, and Gatekeeper checks run unchanged.
Administrator authentication belongs in the terminal, never in chat. A signed install
must finish at `/usr/local/bin/rucksack`.

If no qualifying signed release exists, stop. A development build requires the user's
explicit approval.

## approved development build

Require Xcode Command Line Tools, Rust 1.86 or newer, and a stable source directory. Do not
build in a temporary directory because installed adapter hooks retain the binary's absolute
path.

Clone the official repository, check out the approved exact commit, record its SHA, and run:

```sh
cargo build --workspace --locked
./target/debug/rucksack setup
```

Keep `target/debug/rucksack` beside `target/debug/rucksack-helper`. Describe this path as an
unsigned development build, never as a production security boundary.

## guided setup

Let `rucksack setup` own hotspot or iPhone USB selection, supported-agent detection,
reversible adapter installation, and provider-specific confirmations. Do not request a
hotspot password, change an agent's permissions, or bypass verification with
`--allow-unverified-ssid`, `--allow-unverified-remote`, or `--force`.

The CLI owns the user instructions. Its ownership grammar and product voice live in
[docs/UX.md](docs/UX.md); do not duplicate that script in an install prompt.

## verify

Use the installed binary for every check:

```sh
<rucksack-binary> --version
<rucksack-binary> helper status
<rucksack-binary> adapters status
<rucksack-binary> doctor
```

Warnings remain warnings and failures remain failures. Never run `rucksack pack` as an
installation test.

Report the measured version, binary path, tag or commit, signing status, helper status,
connection mode, adapter status, doctor warnings or failures, and the next command.

For the full trust boundary, read [SECURITY.md](SECURITY.md),
[VALIDATION.md](VALIDATION.md), and [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md).
