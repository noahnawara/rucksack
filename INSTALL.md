# Install rucksack

`rucksack` is currently a source-only alpha. There is no signed GitHub release, Homebrew
cask, or npm package yet, so you build it from source. `cargo` can do that for you.

The helper changes a global macOS sleep setting. Read the [security policy](SECURITY.md)
before installing it, and use only the
[official repository](https://github.com/noahnawara/rucksack).

## Install

Two commands, both in a terminal window.

```sh
cargo install --locked --git https://github.com/noahnawara/rucksack --tag v0.1.0-alpha.4 rucksack-cli rucksack-helper
```

```sh
rucksack helper install
```

Every part of the first command is load-bearing. **Both crate names** must be there: without
`rucksack-helper` the install reports success and `rucksack` can never pack, because the CLI
looks for its helper as a file beside itself. Installing both puts them in the same directory,
so that requirement is satisfied by construction rather than by you remembering to copy two
files. **`--tag`** pins what you get; without it `--git` takes whatever is on the default branch
at the moment you run it, and two people installing an hour apart get different code.
**`--locked`** uses the dependency versions CI actually gates.

It compiles for a couple of minutes. `~/.cargo/bin` is already on your `PATH` if you have Rust,
which you need either way.

The second command is separate because installing the helper needs `sudo`, `sudo` reads your
password from a terminal, and an agent session has no terminal. Doing it here, once, in person,
is what keeps every later `pack` silent. Skip it and an agent's first `pack` fails with:

```text
sudo: a terminal is required to read the password
```

## Requirements

- macOS; release packaging targets macOS 14 and newer
- Xcode Command Line Tools
- Rust 1.86 or newer
- an administrator account for the helper installation
- Codex, Claude Code, or Cursor with its own remote-control feature

Hardware testing so far covers desk runs on one Apple silicon Mac. See
[VALIDATION.md](VALIDATION.md) for the open release checks.

## Build from a clone instead

Use this route if you want a source tree: to run `scripts/e2e.sh`, to work on rucksack, or to
test a commit that is not tagged. Otherwise the two commands above are the shorter path.

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

### Make a cloned build a command

Nothing else in this project says `./target/debug/rucksack`. This guide, the README, and the skill
your agent reads all say `rucksack pack`. So put it where that works: copy **both** binaries into one
directory already on your `PATH`. `~/.cargo/bin` is on it for anyone who has Rust, which you do.

```sh
cp target/debug/rucksack target/debug/rucksack-helper ~/.cargo/bin/
rucksack --version
```

Copy both, and keep them together. `rucksack` finds its helper as a file sitting beside itself, so
symlinking the CLI alone fails with `Build rucksack-helper next to the rucksack binary first` —
macOS reports the symlink's own location rather than the target's. Re-copy after every rebuild;
nothing updates these for you.

## First run

The first successful `pack` also remembers the network it ended up on, and installs a `rucksack`
skill for Codex, Claude Code, and Cursor so "I'm leaving" works inside a conversation. To name the
network yourself:

```sh
rucksack pack --hotspot "My iPhone"
```

## Make departures actually happen

Add these two lines to whatever file your agent always loads — `~/.claude/CLAUDE.md` for Claude Code,
`~/.codex/AGENTS.md` for Codex:

```text
When I say I'm leaving, heading out, or have to go: run `rucksack pack` before carrying on, and
tell me the last line it prints. When I say I'm back: run `rucksack unpack`.
```

Do not skip this, and do not assume the skill covers it. The skill is what makes "pack my Mac" work
as a sentence. It does not reliably make an agent act on "I have to go, catch my train", because a
skill list is consulted when an agent is choosing a tool for the task in front of it, and your
departure is not that task.

That is tested, not supposed. An agent with the skill installed and correctly described was told
"i have to go now, catch my train, keep going on the wordcount thing while i'm out". It kept coding
and never packed the Mac. Asked afterwards, it had read the skill list and simply never compared it
to what the user said.

An always-loaded instruction is part of every conversation, so it does not depend on the agent making
that connection. rucksack writes nothing into these files itself — your instructions to your agents
stay yours.

## Check the installation

```sh
rucksack --version
rucksack helper status
rucksack status
```

Those three are read-only. Do not run `pack` just to test the install: it joins your hotspot, does
not put the previous network back, and takes a real closed-lid lease.

## Use it

```sh
rucksack pack
rucksack status
rucksack unpack
```

`unpack` is also the recovery path. It restores normal sleep from any state, including an
interrupted session or state rucksack can no longer read.

## Remove a development install

Run `unpack` first, then remove the helper:

```sh
rucksack unpack
rucksack helper uninstall
rm ~/.cargo/bin/rucksack ~/.cargo/bin/rucksack-helper
```

## Signed releases

A signed release must contain both `rucksack-universal.pkg` and
`rucksack-universal.pkg.sha256`. The repository's `scripts/install.sh` downloads those
files, checks the SHA-256 checksum, package signature, and Gatekeeper result, then runs the
macOS installer.

Do not run that script until a signed release with both files exists. Administrator
authentication belongs in the terminal, never in a chat.
