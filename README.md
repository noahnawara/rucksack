# Rucksack

**Pack the Mac. Keep the agent.**

Rucksack is a macOS-first, Rust-native handoff ritual for local coding agents. It keeps a
Mac reachable after the lid closes, verifies the iPhone-hotspot transition that normally
breaks the session, and gives Codex, Claude Code, and Cursor a temporary **Commute Mode**
policy designed for limited attention, battery power, and unreliable mobile networking.

```text
$ rucksack pack

Rucksack
Preparing this Mac for the walk home

Agent
✓ Claude Code is running
✓ Remote Control confirmed
✓ Commute policy loaded

Connection
→ Connect “Max’s iPhone”
✓ Online through iPhone

Power
→ Unplug this Mac
✓ Running on battery
✓ Closed-lid lease re-armed
✓ Remote route still healthy

Safety
✓ Battery 78% · sleep at 15%
✓ Thermals normal
✓ Ends in 75 minutes

Ready.
Lock your Mac, close the lid, and go.
Normal sleep will be restored automatically.
```

## The blunt technical truth

Rucksack does **not** fake an AC power connection. macOS obtains AC/battery state from
hardware-backed power-management services. Spoofing that state would require unsupported
private/kernel behavior and would corrupt assumptions used by the rest of the operating
system.

The real failure is usually this:

1. The Mac is awake in closed-display mode while connected to external power.
2. Power is removed.
3. Apple-silicon power management re-evaluates the clamshell state.
4. The Mac sleeps.
5. Wi-Fi and the iPhone hotspot disappear as a consequence.

Rucksack solves the correct problem. A root-owned helper creates a short-lived
`SleepDisabled` lease, re-applies it after power-source changes, and restores a verified
normal baseline when the lease ends. It refuses to start while another utility already
owns `SleepDisabled` or while Amphetamine/`caffeinate` holds an active sleep assertion.
Rucksack never stops those utilities itself. The CLI requires the user to unplug **while
the lid is still open**, verifies the post-transition network path, and only then says it
is safe to close the lid.

See [docs/POWER.md](docs/POWER.md) for the complete reasoning.

## Product status

This repository is a **compiler-verified alpha**. It includes:

- a Rust workspace for the CLI, shared core, and privileged helper;
- a typed JSON protocol between the user process and root helper;
- time-bounded leases with heartbeat expiry and baseline restoration;
- power, battery, thermal, Wi-Fi, route, and internet preflight code;
- native adapter installers for Codex, Claude Code, and Cursor;
- a shared Commute Mode policy with tool-specific hook output;
- an interactive `pack` flow plus `doctor`, `status`, `unpack`, `report`, `recover`, and adapter
  commands;
- macOS/Linux CI, a release-packaging workflow, and a complete product, UX, architecture,
  and threat-model package.

The signed/notarized package pipeline is implemented but has not completed the real-world
release gate. The next milestone is a package validated across Apple-silicon machines,
hotspot modes, and current versions of all three agents.

## Native agent support

| Agent | Remote handoff | Commute policy | State/approval signals | Current limitation |
|---|---|---|---|---|
| Codex | attempts `codex remote-control start`; supports `pair` | user skill plus Codex lifecycle hooks | hooks; App Server is the next richer adapter | when the installed CLI lacks standalone Remote Control, an already-running conversation requires explicit phone confirmation |
| Claude Code | `claude remote-control`, `--remote-control`, or `/remote-control` | skill plus lifecycle hooks | `Notification`, `PermissionRequest`, and `Stop` hooks | an existing interactive session must enable `/remote-control` itself |
| Cursor | Cursor for iOS Remote Control | temporary project rule, `/commute-mode` command, and Cursor hooks | hook telemetry | Remote Control activation/pairing is currently UI-first; hooks are treated as best-effort |

Rucksack does not enable, disable, tighten, or bypass provider permissions. Commute Mode
inherits the active Codex, Claude Code, or Cursor session's permission, approval, and
sandbox configuration exactly. Codex users complete one native trust step after adapter
installation: open `/hooks`, review the marked Rucksack entries, and trust them.

Version 0.1 has no Rucksack-operated backend, relay, or webhook transport. Provider-native
remote products carry the coding conversation.

## Commute Mode behavior

The temporary policy tells the agent to:

- keep pursuing the current acceptance criteria under the task's existing instructions;
- ask only questions that are truly blocking and state reasonable non-blocking assumptions;
- run every workload the task requires, including full builds, broad test suites, Docker,
  VMs, browser automation, and indexing;
- stop and report one clear next action when blocked;
- use bounded retries and surface approval/input waits immediately.

Use `--focus finish`, `--focus investigate`, `--focus review`, or `--focus low-power` to
specialize the policy. Only an explicit `--focus low-power` asks the agent to defer heavy
work.

## Intended command surface

```text
rucksack setup
rucksack setup --hotspot "Max’s iPhone"
rucksack setup --usb
rucksack pack
rucksack pack --agent codex --for 90m --focus finish
rucksack pack --hotspot "Max’s iPhone"
rucksack pack --usb
rucksack status
rucksack unpack
rucksack report
rucksack doctor
rucksack recover

rucksack adapters install
rucksack adapters status
rucksack adapters remove

rucksack pair codex
```

`leave` and `arrive` remain hidden compatibility aliases for `pack` and `unpack`.

Every completed session atomically replaces a private local `last-report.json`. `unpack`
shows that report immediately, and `rucksack report` retrieves it later. Estimated mobile
data is the aggregate download/upload delta for the verified commute interface; it can
include unrelated Mac traffic and is not agent-only attribution or carrier billing.

For Wi-Fi, Rucksack can ask macOS to join a configured network without ever putting a
password in process arguments. A previously saved hotspot can join automatically; Apple
Instant Hotspot may still require selecting the phone in the Wi-Fi menu. An interactive
confirmation that the Wi-Fi menu shows the configured
Instant Hotspot is sufficient evidence when macOS redacts its SSID; the explicit
`--allow-unverified-ssid` flag is only needed to accept a redacted SSID after a successful
exact saved-network join request. `--yes` alone is not interactive evidence. `--usb` is a
separate strict mode: Rucksack waits until `iPhone USB` is the actual default route and
will not mistake ordinary Wi-Fi for wired tethering. Strict hotspot and USB sessions bind
the verified SSID, route interface, and gateway. A different live network releases the
lease immediately; temporary route loss uses the configured reconnect grace.

## Build

Requirements:

- macOS 14 or newer;
- Rust 1.86 or newer;
- an administrator account for one-time helper installation.

```bash
cargo build --workspace --locked
./target/debug/rucksack setup
./target/debug/rucksack doctor
```

The release workflow is designed to produce a signed and notarized universal macOS package.
That package installs and registers the helper through macOS Installer authorization. A
locally compiled release CLI does not install a development helper; use the debug build
above for local helper work. The `rucksack helper install` command is development-only and
refuses to run in release builds.

Published releases use the stable `rucksack-universal.pkg` asset and matching SHA-256 file.
The checksum-verifying `scripts/install.sh` downloads both, validates the package signature
and Gatekeeper assessment, and then invokes the system installer after administrator
authentication. A Homebrew cask remains pending until the first signed release.

For local development, the helper binary must sit next to the debug CLI binary:

```text
target/debug/rucksack
target/debug/rucksack-helper
```

For development without the helper, `doctor` and adapter installation still work; a real
closed-lid session deliberately refuses to start.

## Safety contract

Rucksack promises a **bounded lease**, not “your Mac will never sleep.”

The helper restores normal sleep when any of these occur:

- the user runs `rucksack unpack`;
- the hard session deadline is reached;
- the user daemon stops heartbeating;
- the configured battery floor is reached;
- macOS reports serious/critical thermal pressure or CPU throttling;
- a strict commute route is replaced by a different live SSID, interface, or gateway;
- recovery is requested;
- helper state is inconsistent.

A closed and active laptop can become hot. Rucksack does not restrict builds, VMs,
containers, local models, or other task-required workloads. Independently of the agent
policy, the hardware monitor releases the sleep lease through the helper when macOS reports
serious/critical thermal pressure or CPU throttling, or when the configured battery floor is
reached. CPU utilization alone is not treated as overheating.

## Design documents

- [Product and first principles](docs/PRODUCT.md)
- [User stories and acceptance criteria](docs/USER_STORIES.md)
- [CLI UX](docs/UX.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Power and hotspot model](docs/POWER.md)
- [Native agent adapters](docs/ADAPTERS.md)
- [Threat model](docs/THREAT_MODEL.md)
- [Roadmap](docs/ROADMAP.md)
- [Upstream research](docs/RESEARCH.md)
- [Prior art and attribution](docs/PRIOR_ART.md)
- [Sustainable open-source support](SUPPORT.md)

## Open source

Rucksack is MIT-licensed. The root helper is intentionally small and auditable. Security
reports should follow [SECURITY.md](SECURITY.md); contributions should follow
[CONTRIBUTING.md](CONTRIBUTING.md).
