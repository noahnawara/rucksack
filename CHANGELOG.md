# changelog

all notable changes to rucksack will be documented here.

## [Unreleased]

### Added

- Rust-native CLI, core library, and privileged macOS helper.
- A host-scoped closed-lid lease: it belongs to the Mac, so every running task benefits and
  a task finishing never ends it.
- `pack` with no setup command — the first run installs the helper and remembers the hotspot.
- A resumable hotspot handoff: when macOS cannot join, rucksack opens Wi-Fi settings, says
  what to pick, and waits indefinitely rather than aborting or asking for a re-run.
- Arrival proven by network name, a default-route change, an iOS hotspot gateway, a confirmed
  join, or an explicit `--here` — and only once the route reaches the internet.
- Battery, thermal, hard-deadline, and heartbeat safeguards, with recovery folded into
  `unpack` so restoring normal sleep never dead-ends.
- Thermal pressure read from public `ProcessInfo.thermalState` in the unprivileged watcher.
  `pmset -g therm` reports only Intel-era counters, which Apple silicon never populates, so
  it is kept as a second source rather than the only one.
- A `rucksack` skill for Codex and Claude Code, so "pack my Mac" works in a conversation.
- Universal packaging, signing, and notarization, with a checksum-verifying installer script.
- Canonical `pack`/`unpack` commands; former lifecycle names are rejected.
- rucksack changes no agent instructions, tools, or permissions.

### Changed

- The battery floor is 10%, down from 15%. The old floor ended a commute with a sixth of the
  battery unused, and macOS does not begin its own low-power warnings until 10% either.
- `status` has a battery figure from the first heartbeat. Measuring a rate takes three readings,
  so the opening minutes of a session had only the lease clock to report — the one number certain
  to be wrong, at the moment someone is deciding whether to walk away. macOS has an estimate by
  then and `pmset` is already read every heartbeat, so that figure is borrowed until rucksack has
  measured one of its own, scaled first from time-to-empty to time-to-floor.
- `status` reports whichever limit ends the session first, rather than the lease clock alone. On
  a commute the battery is nearly always the smaller of the two, and a day of lease on an
  afternoon of charge was reported as a day. The battery figure is projected from drain actually
  observed, marked with `~`, and withheld until two drops have been measured.
- `unpack` lets go of the phone. A Mac that arrives somewhere new while still tethered stays on
  the hotspot, because macOS will not abandon a connection that still works; cycling the radio
  makes it choose again from what is in range. Personal Hotspot is a fallback in that choice, so
  a known network nearby wins, and a Mac that is nowhere it knows lands back on the phone.
- The internet probe no longer negotiates TLS to fetch a plain-HTTP page, and every wait now has
  a clock on it.

### Fixed

- `status` no longer reports a dead session as packed.
- The `[adapters]` flags in `config.toml` are read again. `codex = false` had been discarded at
  load, so nothing downstream could obey it.
- Codex is considered installed only when the standalone CLI is on `PATH` or under
  `~/.codex/packages/standalone/current`. The copy inside ChatGPT.app refuses `remote-control`,
  so treating it as usable reported Codex present on almost every Mac and then failed every call.
- An agent this Mac does not have can no longer end a pack. It warns and is skipped; only
  `pack --require-remote` still treats a missing Codex as fatal.
