# rucksack

`rucksack` checks that your Mac can stay awake and online after you unplug, switch to a
phone connection, and close the lid. You continue the same local Codex, Claude Code, or
Cursor session from your phone.

[Website](https://rucksack.wtf) · [Install](INSTALL.md) · [Security](SECURITY.md)

> Source-only alpha. CI passes, and short Codex/hotspot desk tests have passed. There is no
> signed package yet, and all three agent adapters are experimental.

## What it does

Run `rucksack setup` once to install the helper, add the agent adapters, and save your
connection. When you need to leave, `rucksack pack`:

1. Checks the live agent session and walks you through its phone remote.
2. Guides the switch to a Wi-Fi hotspot or iPhone USB.
3. Waits for you to unplug, then checks battery power, the network route, internet access,
   and the agent again.
4. Keeps the Mac awake under a timed lease and watches the connection, battery, and thermal
   state.
5. Adds a temporary commute instruction to the agent, then removes it when the session
   ends.

Codex, Claude Code, or Cursor carries the remote conversation. The `rucksack` CLI and
helper have no backend. Their network checks contain no code, prompts, or command output.
The agent's permission, approval, and sandbox settings stay as you configured them.

## How it works

The Rust workspace has a CLI, shared core, and a small root-owned helper. A normal
`caffeinate` assertion does not cover lid-close sleep on battery, so the helper runs the
fixed command `/usr/bin/pmset -a disablesleep 0|1`.

The helper accepts lease operations over a Unix socket. It cannot run arbitrary commands.
An unprivileged watcher renews the lease while it checks battery level, thermal pressure,
the default route, internet access, and the session deadline. If the watcher dies or a
safety check fails, the helper restores normal sleep.

The agent adapters use each tool's hooks, skills, or rules to add the temporary commute
instruction.

## Build and try it

A development build needs macOS, Xcode Command Line Tools, Rust 1.86 or newer, and
administrator access for the helper. Release packaging targets macOS 14 and newer.

```sh
cargo build --workspace --locked
./target/debug/rucksack setup
./target/debug/rucksack doctor
./target/debug/rucksack pack
./target/debug/rucksack status
./target/debug/rucksack unpack
```

Use `./target/debug/rucksack recover` if a session was interrupted. Read the
[installation guide](INSTALL.md) before installing the helper.

## Safety

A closed Mac running builds or other heavy work can get hot. Test your workload on a
ventilated desk before carrying it.

The lease ends when you unpack, its deadline arrives, the watcher stops heartbeating, the
battery reaches its floor, macOS reports serious thermal pressure or CPU throttling, or a
strict hotspot route is replaced. A long network outage also ends the session after a
short grace period. In each case, `rucksack` restores normal sleep.

`pack` can end early and let the Mac sleep. That is the safe failure mode.

## Documentation

- [Installation](INSTALL.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Agent support](docs/ADAPTERS.md)
- [Security policy](SECURITY.md) and [threat model](docs/THREAT_MODEL.md)
- [Documentation index](docs/README.md)
- [Contributing](CONTRIBUTING.md)

## License

MIT. See [LICENSE](LICENSE).
