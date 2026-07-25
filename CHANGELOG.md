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
- A `rucksack` skill for Codex and Claude Code, so "pack my Mac" works in a conversation.
- Universal packaging, signing, and notarization, with a checksum-verifying installer script.
- Canonical `pack`/`unpack` commands; former lifecycle names are rejected.
- rucksack changes no agent instructions, tools, or permissions.
