# changelog

all notable changes to rucksack will be documented here.

## [Unreleased]

### Added

- Rust-native CLI, core library, and privileged macOS helper.
- Lease-based closed-lid lifecycle with baseline restoration and power-source reassertion.
- Codex, Claude Code, and Cursor native Commute Mode adapters.
- Guided hotspot and AC-to-battery handoff with post-transition verification.
- Battery, thermal, hard-deadline, heartbeat, route-identity, link-loss, and recovery
  safeguards.
- Release-gated universal packaging, signing, notarization, and signed-client
  authentication.
- Stable package asset naming and a checksum-verifying installer script.
- Machine-readable JSON progress and lifecycle state.
- Canonical `pack`/`unpack` commands; former lifecycle names are rejected.
- Private completed-session reports with aggregate commute-interface data estimates.
- Permission-neutral Commute Mode that inherits the active provider session configuration.
- Full task-required workloads by default, with heavy-work deferral only under explicit
  low-power focus and independent release on thermal pressure or CPU throttling.
