# Install rucksack

`rucksack` is currently a source-only alpha. There is no signed GitHub release, Homebrew
cask, npm package, or `cargo install` package yet.

The helper changes a global macOS sleep setting. Read the [security policy](SECURITY.md)
before installing it, and use only the
[official repository](https://github.com/noahnawara/rucksack).

## Requirements

- macOS; release packaging targets macOS 14 and newer
- Xcode Command Line Tools
- Rust 1.86 or newer
- an administrator account for the helper installation
- Codex, Claude Code, or Cursor with its own remote-control feature

The agent adapters are experimental. Current hardware testing covers short Codex/hotspot
desk runs on one Apple silicon Mac. See [VALIDATION.md](VALIDATION.md) for the open release
checks.

## Build from source

Clone the repository into a directory you plan to keep. Adapter hooks store the absolute
path to the development binary, so a temporary checkout will break them later. Check out
the tag or commit you intend to test before recording its hash and building.

```sh
git clone https://github.com/noahnawara/rucksack.git
cd rucksack
git rev-parse HEAD
cargo build --workspace --locked
```

The build puts both required binaries next to each other:

```text
target/debug/rucksack
target/debug/rucksack-helper
```

Run setup from that checkout:

```sh
./target/debug/rucksack setup
```

Setup asks for administrator authorization when it installs the development helper. It
also detects supported agents, installs reversible adapters, and saves the hotspot or
iPhone USB choice.

## Check the installation

```sh
./target/debug/rucksack --version
./target/debug/rucksack helper status
./target/debug/rucksack adapters status
./target/debug/rucksack doctor
```

Use `doctor` as the installation check. Do not run `pack` just to test the install; it
starts a real closed-lid sleep lease.

## Use it

```sh
./target/debug/rucksack pack
./target/debug/rucksack status
./target/debug/rucksack unpack
```

If a session was interrupted, restore normal sleep with:

```sh
./target/debug/rucksack recover
```

## Remove a development install

End or recover any active session first. Then remove the adapters and helper:

```sh
./target/debug/rucksack adapters remove
./target/debug/rucksack helper uninstall
```

## Signed releases

A signed release must contain both `rucksack-universal.pkg` and
`rucksack-universal.pkg.sha256`. The repository's `scripts/install.sh` downloads those
files, checks the SHA-256 checksum, package signature, and Gatekeeper result, then runs the
macOS installer.

Do not run that script until a signed release with both files exists. Administrator
authentication belongs in the terminal, never in a chat.
