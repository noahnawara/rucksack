# Roadmap

## 0.1 — compiler-verified alpha

Implemented in source:

- Rust workspace and typed helper protocol;
- one-time development helper install;
- `pack`, `status`, `unpack`, `report`, `doctor`, `recover`;
- hotspot/unplug handshake;
- battery floor and thermal signal;
- Codex, Claude Code, and Cursor adapter installers;
- temporary policy and hook telemetry;
- waiting/input/approval lifecycle state;
- machine-readable JSON progress;
- private completed-session reports with aggregate commute-interface data estimates;
- strict hotspot/USB route identity with reconnect grace for temporary loss;
- macOS/Linux CI and parser/merge tests;
- release-gated universal packaging, signing/notarization workflow, and signed-client
  helper authorization;
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
- persisted per-provider remote onboarding: installation, pairing, native trust, and baseline
  phone visibility are confirmed once during `setup`; later `pack` runs still bind and
  activate the exact live session, but repeat onboarding proof only when the provider
  account, paired device, adapter/trust state, or detected provider capability materially
  changes;
- make the helper's 24-hour safety horizon the default hard deadline; keep `rucksack pack
  --for <duration>` for intentionally shorter sessions and keep active hard deadlines
  non-renewable;
- commute-aware orchestration: keep useful independent work parallel, but give every
  agent, monitor, preview server, and poll loop an owner, purpose, stop condition, and
  cleanup path; prefer event-driven waits and cached watcher evidence, and terminate work
  promptly when it completes or becomes irrelevant;
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
