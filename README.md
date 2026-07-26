# rucksack

Close your laptop and keep your agents running.

`rucksack` is a macOS command-line tool for leaving your desk while a local coding agent is
still working. It moves your Mac onto your phone hotspot, verifies the handoff, and holds a
bounded lease that keeps the Mac awake with the lid closed. You keep steering the same
Codex, Claude Code, or Cursor session from your phone. Switching to the hotspot is one
step, where moving the work to a cloud agent means standing up a fresh environment.

[Website](https://www.rucksack.wtf/) · [Install](INSTALL.md) · [Security](SECURITY.md)

> Source-only alpha. CI passes, and short hotspot desk tests have passed. There is no signed
> package yet, and closed-lid behaviour in a bag is not yet verified.

## Install

```sh
cargo install --locked --force --git https://github.com/noahnawara/rucksack --tag v0.1.0-alpha.4 rucksack-cli rucksack-helper
rucksack helper install
```

Both crate names are required. `rucksack` finds its helper as a file beside itself, so installing
only `rucksack-cli` reports success and can never pack. Installing both puts them in one directory.

Run `helper install` yourself, in a terminal. `pack` would do it on first use, but it needs `sudo`,
and `sudo` reads your password from a terminal that an agent session does not have. Needs macOS,
Xcode Command Line Tools, and Rust 1.86 or newer; release packaging targets macOS 14 and newer.
Read the [installation guide](INSTALL.md) for the clone-and-build route and removal.

Or paste this to Codex, Claude Code, or Cursor and let it do the first part:

```text
Install rucksack on this Mac by running exactly this, both crate names included:

cargo install --locked --force --git https://github.com/noahnawara/rucksack --tag v0.1.0-alpha.4 rucksack-cli rucksack-helper

Without `rucksack-helper` the install reports success and `rucksack` can never pack, so don't drop
it. It compiles for a couple of minutes and needs nothing added to my PATH. Don't run `rucksack
pack` to test it. Then tell me to run `rucksack helper install` myself in a terminal window — it
asks for my password once, and getting it out of the way now means no later `rucksack pack` ever
stops for it. Don't try to run it for me: you have nowhere to type a password.
```

## What it does

There is no setup command. The first `pack` installs the power helper and remembers the
network it ended up on.

Run that first one yourself, in a terminal — `rucksack helper install` does the same job on its
own. Installing the helper needs your password, `sudo` asks for it on a terminal, and the agent
session that will run every later `pack` has no terminal to be asked on. Doing it once, in
person, is what keeps every `pack` after it silent.

```text
$ rucksack pack

Connecting to Noah…
Joined.
Awake for 24 hours, or until the battery hits 15%. Ends 09:14 tomorrow.
Packed. Close the lid and go.
```

If macOS cannot join the hotspot itself — the normal case for Apple Instant Hotspot —
rucksack opens Wi-Fi settings, says what to pick, and waits:

```text
Choose “Noah” in Wi-Fi — keep the lid open until this says Packed. Waiting…
```

It then carries on by itself. There is nothing to confirm and nothing to re-run. The lid clause
is the one thing worth reading twice: until `pack` says `Packed`, nothing is holding this Mac
awake yet, so closing it during the wait sleeps the machine.

The lease belongs to the **Mac**, not to one conversation. Closing the lid affects every
process, so every running task benefits, and a task finishing does not end the lease.

rucksack does not change how your agents behave. It writes no instructions, prompts, or
policy into them, and installs no hooks; your permission, approval, and sandbox settings
stay exactly as you configured them. It installs one thing: a `rucksack` skill for Codex,
Claude Code, and Cursor, so "I'm leaving" works as a sentence in a conversation. Whichever of
them you use carries the remote conversation. The CLI and helper have no backend.

When you are back at a desk, `rucksack unpack` restores normal sleep, and says what the trip
was. It is also the recovery path, and works from any state.

```text
$ rucksack unpack

Packed for 3h 12m · battery 79% → 61% · 240 MB
Still on your phone. Pick a Wi-Fi network when you can.
Unpacked. This Mac sleeps normally.
```

Each of those numbers is measured or absent. A battery gauge that says nothing, a byte counter
that reset, or an interface that changed mid-trip all shorten that line rather than fill it in
with something plausible. The byte figure spans the watcher's first heartbeat to its last, so it
reads slightly low rather than slightly high.

## The whole command surface

```text
rucksack pack     switch to your hotspot and hold this Mac awake
rucksack status   is it still packed?
rucksack unpack   let this Mac sleep again
rucksack pair     print a Codex Remote Control pairing code
rucksack star     star this project on GitHub
rucksack helper   install or remove the power helper
```

`pack` takes `--for 90m` to shorten the session, `--hotspot "Name"` to name the network,
`--usb` for iPhone USB tethering, and `--here` when you are already on the network you want.

## Why caffeinate is not enough

`caffeinate` keeps a Mac awake for as long as it runs. `caffeinate -d` also holds the
display on, and `caffeinate -i make` keeps the machine up until that command finishes. With
the lid open, on power or on battery, it does what you want and rucksack has nothing to add.

Close the lid and it stops helping. A `caffeinate` assertion prevents *idle* sleep, and
closing the lid on battery is a *forced* sleep condition evaluated by closed-display policy,
so the assertion does not cover it. That is the whole gap: `caffeinate` is useful
defense-in-depth while the lid is open, and it is not the closed-lid primitive.

The setting that does cover it is `SleepDisabled`, and it is global, persistent, and
root-only. Nothing releases it when a process exits, so leaving it on is how a Mac ends up
permanently unable to sleep. rucksack takes that stronger setting and gives it the thing
`caffeinate` had for free: an owner, and an end.

rucksack does not run, wrap, or extend `caffeinate`. It refuses to start when `SleepDisabled`
is already on, because that value is the baseline it has to hand back, and two owners make
cleanup ambiguous. rucksack never stops or modifies whatever holds it.

## The network half

Two things end a session when you walk away, not one. The Mac sleeps, and the default route
changes. Keeping the Mac awake solves the first and leaves the second, which is why a
session can survive the lid and still die at the door.

So `pack` treats the route as a precondition, not an afterthought — and "the internet works"
is not the test, because the office network you are walking away from also works. rucksack
accepts a network only once it can show the Mac has actually moved onto the commute one: by
its name, by an iPhone hotspot gateway, by the default route visibly leaving where it
started, or because you said so with `--here`. Then it checks that the route really reaches
the internet, requiring Apple's own captive-network success page so a hotel portal cannot
pass.

Losing that network later does **not** end the session. A tunnel is exactly when your work
most needs to stay alive, so an outage is reported by `status` and nothing else.

## How it works

The Rust workspace has a CLI, shared core, and a small root-owned helper. The helper runs
one fixed command, `/usr/bin/pmset -a disablesleep 0|1`, and cannot run anything else. It
refuses to acquire unless normal sleep is the verified baseline, saves that baseline before
changing it, and verifies the result afterward.

The helper accepts lease operations over a Unix socket. An unprivileged watcher renews the
lease while it checks battery level, thermal pressure, and the session deadline. The watcher
cannot change the sleep setting itself. If it dies, the lease expires on its own and the
helper restores normal sleep.

## How rucksack compares

Running `sudo pmset -a disablesleep 1` by hand sets the same value, with no saved baseline,
no expiry, and no release when the battery or the temperature says stop. It survives a
crash and a reboot as a machine that no longer sleeps.

Keep-awake apps such as Amphetamine and KeepingYouAwake hold the setting for as long as you
leave them on. They are the right tool when you are staying. rucksack is the one you run
when you are leaving: it is a command, it is bounded, and it checks the network as well as
the power state.

Cloud agents and remote dev environments solve a different problem. A cloud run starts from
a clean checkout, while rucksack keeps the session that already has your working tree, your
uncommitted changes, your local environment, and your permission and sandbox settings.

Provider remotes carry the phone-side conversation: Claude Code's remote control and the
Codex and Cursor equivalents. rucksack exists to keep them reachable, since they have
nothing to talk to once the Mac is asleep or off the network.

See [prior art](docs/PRIOR_ART.md) for related projects.

## Safety

A closed Mac running builds or other heavy work can get hot. Test your workload on a
ventilated desk before carrying it.

The lease ends for host-level reasons only:

- you run `unpack`
- the time limit arrives
- the battery reaches its floor (15% by default)
- macOS reports serious thermal pressure or actual throttling
- the battery gauge cannot be read three times in a row while on battery
- the helper stops answering, and its own TTL restores sleep

In each case, `rucksack` restores normal sleep. An agent finishing its task does not end the
session, and neither does losing the network.

If `pack` cannot finish, it rolls back and lets the Mac sleep. That is the safe failure mode.

## Documentation

- [Installation](INSTALL.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Agent support](docs/ADAPTERS.md)
- [Security policy](SECURITY.md) and [threat model](docs/THREAT_MODEL.md)
- [Documentation index](docs/README.md)
- [Contributing](CONTRIBUTING.md)

## License

MIT. See [LICENSE](LICENSE).
