# rucksack

Close your laptop and keep your agent running.

`rucksack` is a macOS command-line tool for leaving your desk while a local coding agent is
still working. It moves your Mac onto your phone hotspot, verifies the handoff, and holds a
bounded lease that keeps the Mac awake with the lid closed. You keep steering the same
Codex, Claude Code, or Cursor session from your phone. Switching to the hotspot is one
step, where moving the work to a cloud agent means standing up a fresh environment.

[Website](https://www.rucksack.wtf/) · [Install](INSTALL.md) · [Security](SECURITY.md)

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

## Why caffeinate is not enough

A `caffeinate` assertion prevents *idle* sleep. Closing the lid on battery is a *forced*
sleep condition evaluated by closed-display policy, so the assertion does not cover it.
`caffeinate` is useful defense-in-depth while the lid is open; it is not the closed-lid
primitive.

The setting that does cover it is `SleepDisabled`, and it is global, persistent, and
root-only. Nothing releases it when a process exits, so leaving it on is how a Mac ends up
permanently unable to sleep. rucksack takes that stronger setting and gives it the thing
`caffeinate` had for free: an owner, and an end.

rucksack does not run, wrap, or extend `caffeinate`. An active `caffeinate` or Amphetamine
assertion blocks readiness, because two owners of the sleep setting make cleanup ambiguous.
rucksack never stops or modifies them.

## The network half

Two things end a session when you walk away, not one. The Mac sleeps, and the default route
changes. Keeping the Mac awake solves the first and leaves the second, which is why a
session can survive the lid and still die at the door.

So `pack` treats the route as a precondition, not an afterthought. It confirms the hotspot
network, checks the default route and real internet reachability, re-asserts after you
unplug, and re-checks the provider endpoint before it reports `packed`. The watcher keeps
comparing the live route against the pinned one for the rest of the session.

## How it works

The Rust workspace has a CLI, shared core, and a small root-owned helper. The helper runs
one fixed command, `/usr/bin/pmset -a disablesleep 0|1`, and cannot run anything else. It
refuses to acquire unless normal sleep is the verified baseline, saves that baseline before
changing it, and verifies the result afterward.

The helper accepts lease operations over a Unix socket. An unprivileged watcher renews the
lease while it checks battery level, thermal pressure, the default route, internet access,
and the session deadline. The watcher cannot change the sleep setting itself. If it dies or
a safety check fails, the helper restores normal sleep.

The agent adapters use each tool's hooks, skills, or rules to add the temporary commute
instruction.

## How rucksack compares

Running `sudo pmset -a disablesleep 1` by hand sets the same value, with no saved baseline,
no expiry, and no release when the battery or the temperature says stop. It survives a
crash and a reboot as a machine that no longer sleeps.

Keep-awake apps such as Amphetamine and KeepingYouAwake hold the setting for as long as you
leave them on. They are the right tool when you are staying. rucksack is the one you run
when you are leaving: it is a command, it is bounded, and it checks the network and the
agent as well as the power state.

Cloud agents and remote dev environments solve a different problem. A cloud run starts from
a clean checkout, while rucksack keeps the session that already has your working tree, your
uncommitted changes, your local environment, and your permission and sandbox settings.

Provider remotes carry the phone-side conversation: Claude Code's remote control and the
Codex and Cursor equivalents. rucksack exists to keep them reachable, since they have
nothing to talk to once the Mac is asleep or off the network.

See [prior art](docs/PRIOR_ART.md) for related projects.

## Install

There is no signed package yet, so you build from source. A development build needs macOS,
Xcode Command Line Tools, Rust 1.86 or newer, and administrator access for the helper.
Release packaging targets macOS 14 and newer.

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
- [Power and hotspot behavior](docs/POWER.md)
- [Agent support](docs/ADAPTERS.md)
- [Security policy](SECURITY.md) and [threat model](docs/THREAT_MODEL.md)
- [Prior art and attribution](docs/PRIOR_ART.md)
- [Documentation index](docs/README.md)
- [Contributing](CONTRIBUTING.md)

## License

MIT. See [LICENSE](LICENSE).
