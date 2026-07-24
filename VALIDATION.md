# Validation status

Validation date: **2026-07-24**.

This source tree has been compiled and tested on macOS. The compiler gate below passes,
including strict Clippy and release-helper checks with a dummy CI Team ID. A real
privileged-helper and short closed-lid smoke test have passed; the signed package,
15-minute closed-lid gate, and broader hardware/provider matrix remain release gates. The
project must not be described as production-ready until those checks pass.

## Current source inventory

Counts include repository dotfiles and exclude `.git` and `target`:

- 33 Rust source files;
- 11 TOML files;
- 8 JSON files;
- 1 launchd plist;
- 4 YAML workflow/configuration files, including 2 GitHub Actions workflows;
- 57 Markdown files.

## Completed automated checks

The current tree passes:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUCKSACK_TEAM_ID=CI00000000 \
  cargo clippy --workspace --all-targets --release --locked -- -D warnings
RUCKSACK_TEAM_ID=CI00000000 \
  cargo test --workspace --release --locked
RUCKSACK_TEAM_ID=CI00000000 \
  cargo build --workspace --release --locked
cd site && npm audit --audit-level=high
cd site && npm run build
cd site && npm run test:e2e
```

Test results:

- 183 debug tests: CLI 75, CLI JSON E2E 8, core 89, helper 11;
- 184 release tests with the dummy CI Team ID: CLI 75, CLI JSON E2E 8, core 89,
  helper 12;
- 10 browser tests and 0 high-severity npm vulnerabilities;
- 0 failures.

The automated and structural review also verifies:

- helper protocol version 2 and a persisted non-renewable hard deadline;
- baseline persistence before mutation and refusal to acquire over an existing
  `SleepDisabled` owner;
- owner/root authorization for active lease mutation and recovery;
- release-build client identifier, Developer ID chain, and Team ID requirements;
- fixed absolute `pmset -a disablesleep 0|1` execution with cleared environment and
  bounded time/output;
- bounded helper connections and request I/O;
- ambiguous helper-acquire responses trigger lease cleanup unless an authoritative status
  proves that cleanup is unnecessary;
- stale/corrupt helper-state recovery to ordinary sleep;
- rollback of helper lease, temporary policy, Cursor files, and watcher state;
- one-time provider confirmation tokens bind policy delivery to the exact active agent,
  project, and provider session;
- a strict provider-scoped onboarding registry stores only measured/user-confirmed
  evidence, timestamps, typed invalidation reasons, and SHA-256 bases under `0700`/`0600`
  ownership checks;
- pairing, native-trust, and baseline phone-visibility evidence survives unchanged packs,
  while adapter changes and explicit re-pairing invalidate only the affected provider
  evidence;
- `--allow-unverified-remote` can bypass missing phone-onboarding or endpoint evidence but
  cannot bypass the fresh exact-task provider binding;
- inactive cleanup locators survive Cursor cleanup failures without exposing policy text;
- Cursor activation, rollback, and cleanup use held directory descriptors, bounded reads,
  and transactional writes so path or symlink swaps cannot redirect managed operations;
- atomic, ownership-aware configuration merges that preserve unrelated entries;
- bounded captive-portal and provider probes that contain no repository data;
- strict hotspot/USB route binding, immediate release on confirmed route replacement, and
  reconnect grace for route loss;
- serious/critical thermal pressure or CPU/scheduler throttling releases the lease and
  records the observed limits, while raw CPU utilization alone does not false-trigger;
- canonical `pack`, `unpack`, and `report` commands with the former lifecycle names
  rejected;
- atomic, user-only completed-session reports with aggregate interface-counter deltas that
  become partial/unavailable rather than fabricating zero, restore sleep before final
  accounting, serialize pack/finalization commands, preserve newer session state, and reject
  stale-session replacement;
- a stable `rucksack-universal.pkg` release asset plus checksum, signature, and Gatekeeper
  validation in `scripts/install.sh`;
- repository-relative Markdown links.

`CI00000000` exercises release-only authorization code paths; it is not a production
signing identity and does not validate a signed package.

## Supervised hardware observation

On 2026-07-24, a MacBook Air (`Mac17,4`, Apple M5) running macOS 26.5.2 completed a
supervised short smoke test with an iPhone Personal Hotspot:

- the ordinary Wi-Fi route (`en0`, gateway `192.168.1.1`) changed to the hotspot route
  (`en0`, gateway `192.0.0.1`);
- macOS returned exit code 0 while printing `Could not find network Noah.` for the
  command-line join, and Rucksack correctly treated it as a failure before requesting the
  manual Instant Hotspot selection;
- internet and Codex provider checks passed both before and after the AC-to-battery
  transition;
- the helper reported the saved baseline `SleepDisabled=0`, active
  `SleepDisabled=1`, and a non-renewable hard deadline;
- with `AppleClamshellState=Yes`, local daemon revisions, helper renewals, the pinned route,
  and provider health advanced for approximately three minutes;
- after the lid was reopened for an unrelated call, `rucksack unpack` stopped the watcher
  and restored `SleepDisabled=0`; the Mac then returned to the ordinary Wi-Fi route.

A second wireless-only run then exercised the missing route-return path:

- no iPhone USB network interface was active; the hotspot remained the `en0` default route
  with gateway `192.0.0.1`;
- the manual Instant Hotspot selection survived the battery transition, and the development
  flow exposed a redundant `--allow-unverified-ssid` requirement that has now been removed
  for explicit interactive Wi-Fi-menu confirmation;
- with `AppleClamshellState=Yes` and `SleepDisabled=Yes`, three consecutive daemon revisions
  and heartbeats advanced while captive-network probes continued to pass;
- selecting ordinary Wi-Fi changed the gateway to `192.168.1.1`; the watcher recorded that
  exact route replacement, automatically released the lease, and restored
  `SleepDisabled=0`.

This is positive wireless-only and automatic-release smoke evidence, not the required
15-minute hardware gate.

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

Complete the matrix in `docs/POWER.md` on at least:

- one current Apple-silicon MacBook on the oldest supported macOS release;
- one current Apple-silicon MacBook on the newest supported macOS release;
- Wi-Fi Personal Hotspot and USB tethering;
- Codex, Claude Code, and Cursor at their current stable versions.

The test must include:

1. AC power connected, hotspot connected, remote visible on the phone;
2. AC→battery transition **with the lid open**;
3. post-transition `SleepDisabled`, route, captive-network, and provider verification;
4. lid closure and at least a 15-minute remote session;
5. battery-floor release;
6. thermal release or a controlled thermal-state fixture;
7. user-daemon termination;
8. helper termination/restart;
9. reboot with persisted helper state;
10. `unpack`, `recover`, helper upgrade, and helper uninstall;
11. exact restoration of normal sleep after every case.

No release should claim production readiness until package, integration, and real-hardware
checks all pass.
