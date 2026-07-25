# Roadmap

## 0.1 — compiler-verified alpha

Implemented in source:

- Rust workspace and typed helper protocol;
- `pack`, `status`, `unpack`, `pair`, `star`, and `helper install`/`status`/`uninstall`;
- helper install on the first `pack` that needs it, so macOS authenticates once;
- arrival on a commute network proven by the saved network name, the default route leaving
  its baseline interface or gateway, the 172.20.10.1 iOS Personal Hotspot gateway, a join
  macOS confirmed, or `--here` — and in every case an internet probe on that route;
- waiting for as long as it takes when macOS cannot join, ticking every 30 seconds, with
  no abort and no re-run;
- a host-scoped lease that survives later network loss;
- battery floor and thermal signal;
- a 24-hour default hard deadline with shorter sessions available through `--for`;
- `unpack` restoring normal sleep from any state, including session state it cannot parse;
- one marker-guarded skill file for Codex and Claude Code;
- macOS and Linux CI with parser and state tests;
- release-gated universal packaging and a signing/notarization workflow, with helper
  authorization that verifies the caller's Apple code signature only when the helper was
  built with a team id, and authenticates by peer UID alone otherwise;
- stable `rucksack-universal.pkg` assets and checksum-verifying installer script.

Release gate:

- 50 supervised desk tests across at least three Apple-silicon models;
- zero machines left sleep-disabled after injected daemon/helper crashes;
- configuration uninstall is lossless;
- every “Packed” session survives a 15-minute lid-closed test;
- a production-credential package passes signing, notarization, install, upgrade, and
  uninstall verification.

## 0.2 — signed field beta

- public `ProcessInfo.thermalState` FFI;
- IOKit battery data instead of command parsing;
- provider/version capability detection;
- optional explicitly configured return Wi-Fi for `unpack`; never guess from the user's
  preferred-network list;
- Homebrew cask after the first signed release;
- migration path from the Node implementation.

Exit criteria:

- 200 real commutes;
- restoration success above 99.9%;
- explicit list of supported macOS and agent versions;
- no P0/P1 security findings from helper review.

## 0.3 — excellent native adapters

- Codex App Server status integration;
- Claude session URL/status extraction where stable;
- Cursor machine-readable Remote Control integration if published;
- per-agent test fixtures and compatibility matrix;
- adapter health self-test.

## 0.4 — companion status

- local encrypted relay or provider-native deep links only;
- optional iOS/Watch companion for health, not code transport;
- view/end lease;
- battery/thermal/link notifications;
- multiple Macs.

Any companion relay remains operational-metadata only: no repository content or
transcripts.

## 1.0 — boring reliability

- signed stable installer/updater;
- crash/reboot/power-transition certification matrix;
- deterministic rollback;
- full threat-model review;
- localization-ready CLI copy;
- accessibility-tested terminal output;
- documented API for third-party agent adapters.

## Later — execution handoff

The larger product is moving work from the laptop to a devbox/cloud environment and
letting the Mac sleep normally:

```text
local agent
  → clean checkpoint/worktree
  → remote environment verification
  → resume agent remotely
  → verify phone control
  → release local lease
```

This should be built only after host-side reliability is boring. It is not a reason to
turn Rucksack into a generic orchestration platform prematurely.
