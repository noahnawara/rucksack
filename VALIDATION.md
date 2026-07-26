# Validation status

Validation date: **2026-07-26**.

This source tree has been compiled and tested on macOS. The compiler gate below passes,
including strict Clippy and the release profile. A real privileged helper and a short
closed-lid smoke test have passed on hardware; the signed package, the 15-minute
closed-lid gate, and a broader hardware matrix remain release gates. The project must not
be described as production-ready until those checks pass.

## Current source inventory

Counts include repository dotfiles and exclude `.git`, `target`, `site/node_modules`, and
`site/test-results`:

- 25 Rust source files;
- 8 TOML files;
- 4 JSON files;
- 1 launchd plist;
- 4 YAML configuration files, including 2 GitHub Actions workflows;
- 23 Markdown files.

## Completed automated checks

The current tree passes:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo clippy --workspace --all-targets --release --locked -- -D warnings
cargo test --workspace --release --locked
cargo build --workspace --release --locked
cd site && npm audit --audit-level=high
cd site && npm run build
cd site && npm run test:e2e
```

Test results:

- 100 debug tests: CLI 36, CLI contract 7, core 46, helper 11;
- 101 release tests: the same set plus one macOS release-only helper test;
- 43 end-to-end assertions from `scripts/e2e.sh`, against real leases on this hardware;
- 16 browser tests and 0 vulnerabilities reported by `npm audit`;
- 0 failures.

## What the checks certify

The tests and the structural review cover the invariants that make a closed lid safe:

- the lease is host-scoped: it belongs to the Mac, not to a conversation, task, agent, or
  provider session, and no agent-facing mechanism exists that could end it;
- losing the network never releases the lease, and neither does an agent finishing its
  work; both are recorded, and the internet probe only feeds `status`;
- the lease ends by itself for exactly these reasons: explicit `unpack`, the hard
  deadline, the battery floor, serious or critical thermal pressure or actual throttling,
  three consecutive unreadable battery levels while on battery, and a helper that stops
  answering the heartbeat, after which the helper's own TTL restores sleep;
- the release decision is a pure function, so "a healthy Mac keeps its lease" is a test
  rather than a hope, including a silent battery gauge on AC power;
- `pack` proves it reached a commute network instead of accepting that the internet works:
  the Wi-Fi name matching the saved hotspot, the default route leaving its baseline
  interface or gateway, an iOS Personal Hotspot gateway (`172.20.10.1`, or `192.0.0.1` on
  an IPv6-only carrier), a join macOS confirmed, or the user passing `--here` — and in
  every case the route must then reach the internet;
- a working office network is refused, because accepting it would pack a Mac that goes
  offline at the front door;
- when macOS cannot join, `pack` prints one instruction, opens Wi-Fi settings, and waits
  for as long as it takes, resuming by itself once the network arrives; it never aborts
  and never asks for a re-run, and it takes no lease while waiting;
- `pack` exposes no confirmation, onboarding, or override flags: `--yes`, `--agent`,
  `--focus`, and `--allow-unverified…` are gone, the global `--json` flag is gone, and
  `setup`, `doctor`, `report`, `recover`, `adapters`, and `hook` no longer exist;
- helper protocol version 2 and a persisted hard deadline that a renewal cannot move;
- the rollback target is persisted before the global power setting changes, and the
  helper refuses to acquire over an existing `SleepDisabled` owner;
- owner-or-root authorization for mutating and recovering an active lease;
- callers are authenticated by peer UID through `getpeereid`, behind a `root:admin`
  `0660` socket, with per-lease owner checks. A helper compiled with an Apple Team ID
  additionally pins the calling binary's code signature to the `io.rucksack.cli`
  identifier, that team, and a Developer ID chain — that is what signed release packages
  do. A helper built from source has no team id, says so on startup, and authenticates by
  UID alone;
- fixed absolute `/usr/bin/pmset -a disablesleep 0|1` execution with a cleared environment
  and bounded time and output;
- bounded helper connections and request I/O;
- an acquire that answers without a status is treated as a failure, and `pack`'s rollback
  then stops the watcher, releases the lease, and clears session state;
- stale or corrupt persisted helper state recovers to ordinary sleep;
- `unpack` absorbs recovery: release by lease id, helper recover by owner uid, then
  accepting macOS's own `SleepDisabled=0`. Session state it cannot parse is reported,
  deleted, and never dead-ends into a second command;
- `unpack` says only what it did: releasing its own lease, releasing a standing lease no
  session file names, and finding sleep already normal are three separate outcomes. The
  helper answers `recover` identically whether it released a lease or found none, so
  whether one was standing is read before recovering, and a Mac with nothing packed is
  never told a lease was let go;
- atomic `0600` writes for configuration and session state, and a configuration written
  by an older release still loads with retired keys ignored;
- a bounded captive-portal probe that carries no repository data;
- the skill installs only where the agent already lives, replaces the retired
  `commute-mode` skill, refuses to overwrite a file rucksack does not own, and can never
  fail `pack`;
- a stable `rucksack-universal.pkg` release asset plus checksum, `pkgutil` signature, and
  `spctl` Gatekeeper validation in `scripts/install.sh`.

## Real-lease end-to-end script

`scripts/e2e.sh` drives the CLI the way a person drives it, against real power leases: it
switches system sleep off and back on, runs every case against a throwaway `HOME`, and
always restores normal sleep on exit, including on failure or interrupt. Run it by hand on
a Mac — `scripts/e2e.sh [path-to-rucksack]`, defaulting to `target/debug/rucksack`. It
checks:

- `--help` takes no lease, and `setup`, `doctor`, `report`, `recover`, `adapters`, and
  `hook` are gone from it;
- `status` on a fresh Mac is one line, and `unpack` with nothing packed still succeeds;
- contradictory or out-of-range `pack` arguments are refused and take no lease;
- unreadable session state survives `status` and is cleared by `unpack`;
- a configuration from an older release still loads;
- `pack` on ordinary Wi-Fi refuses to accept that network, keeps waiting rather than
  aborting, never claims success, takes no lease, and leaves sleep normal when interrupted;
- the lease lifecycle: `pack --here` succeeds without questions in a few lines, sleep is
  actually switched off, `status` says Packed, the lease keeps standing on its own with no
  agent involvement, a second `pack` refuses without disturbing the first lease, and
  `unpack` restores normal sleep;
- a second `unpack` in a row says "Already unpacked", claims no release, and leaves sleep
  normal, against a real helper that is installed and holding nothing;
- `pack` installs the `rucksack` skill and retires `commute-mode`.

## Supervised hardware observation (historical)

These runs are from 2026-07-24 and predate the current release rules. Read them as
evidence that the helper, the lease, and the manual hotspot handoff work on real hardware
— not as a description of today's behaviour.

A MacBook Air (`Mac17,4`, Apple M5) running macOS 26.5.2 completed a supervised short
smoke test with an iPhone Personal Hotspot:

- the ordinary Wi-Fi route (`en0`, gateway `192.168.1.1`) changed to the hotspot route
  (`en0`, gateway `192.0.0.1`);
- macOS returned exit code 0 while printing `Could not find network Noah.` for the
  command-line join, and rucksack correctly treated it as a failure before requesting the
  manual Instant Hotspot selection;
- internet checks passed both before and after the AC-to-battery transition;
- the helper reported the saved baseline `SleepDisabled=0`, active `SleepDisabled=1`, and a
  hard deadline a renewal could not move;
- with `AppleClamshellState=Yes`, watcher revisions and helper renewals advanced for
  approximately three minutes;
- after the lid was reopened for an unrelated call, `rucksack unpack` stopped the watcher
  and restored `SleepDisabled=0`; the Mac then returned to the ordinary Wi-Fi route.

A second wireless-only run held the lease with no iPhone USB interface active, and the
manual Instant Hotspot selection survived the AC-to-battery transition while heartbeats
continued to advance with the lid closed. That run also observed an automatic release when
the Mac returned to ordinary Wi-Fi; rucksack no longer behaves that way, because a network
change is recorded and never ends a lease.

This is positive wireless-only smoke evidence, not the required 15-minute hardware gate.

## Required package gate

Before publishing a tag:

1. build universal binaries with the production Apple Team ID;
2. sign the CLI, helper, and installer with the intended Developer ID identities;
3. notarize, staple, and assess the package;
4. publish `rucksack-universal.pkg` and its checksum under the stable GitHub Release names;
5. install through `scripts/install.sh`, then upgrade and uninstall on a clean supported
   Mac;
6. verify the live helper accepts only the signed CLI and restores normal sleep.

## Required hardware gate

Complete the cases below on at least:

- one current Apple-silicon MacBook on the oldest supported macOS release;
- one current Apple-silicon MacBook on the newest supported macOS release;
- Wi-Fi Personal Hotspot and USB tethering;
- Codex Remote Control at its current stable version.

The test must include:

1. AC power connected and the hotspot connected;
2. AC→battery transition **with the lid open**;
3. post-transition `SleepDisabled`, route, and captive-network verification;
4. lid closure and at least a 15-minute session;
5. battery-floor release;
6. thermal release or a controlled thermal-state fixture;
7. watcher termination;
8. helper termination/restart;
9. reboot with persisted helper state;
10. `unpack`, helper upgrade, and helper uninstall;
11. exact restoration of normal sleep after every case.

No release should claim production readiness until package, integration, and real-hardware
checks all pass.
