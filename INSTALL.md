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

Hardware testing so far covers desk runs on one Apple silicon Mac. See
[VALIDATION.md](VALIDATION.md) for the open release checks.

## Build from source

Clone the repository into a directory you plan to keep. Check out the tag or commit you
intend to test before recording its hash and building.

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

There is no setup command. `rucksack pack` installs the power helper the first time it runs,
which is when macOS asks for administrator authorization.

To get that prompt out of the way now instead:

```sh
./target/debug/rucksack helper install
```

The first successful `pack` also remembers the network it ended up on, and installs a `rucksack`
skill for Codex and Claude Code so "pack my Mac" works inside a conversation. To name the network
yourself:

```sh
./target/debug/rucksack pack --hotspot "My iPhone"
```

## Check the installation

```sh
./target/debug/rucksack --version
./target/debug/rucksack helper status
./target/debug/rucksack status
```

Those three are read-only. Do not run `pack` just to test the install: it joins your hotspot, does
not put the previous network back, and takes a real closed-lid lease.

## Use it

```sh
./target/debug/rucksack pack
./target/debug/rucksack status
./target/debug/rucksack unpack
```

`unpack` is also the recovery path. It restores normal sleep from any state, including an
interrupted session or state rucksack can no longer read.

## Remove a development install

Run `unpack` first, then remove the helper:

```sh
./target/debug/rucksack unpack
./target/debug/rucksack helper uninstall
```

## Signed releases

A signed release must contain both `rucksack-universal.pkg` and
`rucksack-universal.pkg.sha256`. The repository's `scripts/install.sh` downloads those
files, checks the SHA-256 checksum, package signature, and Gatekeeper result, then runs the
macOS installer.

Do not run that script until a signed release with both files exists. Administrator
authentication belongs in the terminal, never in a chat.
