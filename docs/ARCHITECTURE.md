# Architecture

## Components

```text
┌─────────────────────────────────────────────────────────────┐
│ rucksack CLI                                                │
│ setup · doctor · pack · status · unpack · report · recover  │
└───────────────┬─────────────────────────────────────────────┘
                │ typed JSON over Unix socket
┌───────────────▼─────────────────────────────────────────────┐
│ rucksack-helper (root LaunchDaemon)                         │
│ signed-client auth · lease · pmset · power events · watchdog│
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│ rucksack daemon (user)                                      │
│ heartbeats · battery · thermal · route · probes · policy    │
└───────────────┬─────────────────────────────────────────────┘
                │ files / hook stdio
┌───────────────▼─────────────────────────────────────────────┐
│ Codex hooks + skill · Claude hooks + skill · Cursor hooks   │
│ + temporary rule                                            │
└─────────────────────────────────────────────────────────────┘
```

## Crates

### `rucksack-core`

No privilege. Shared types and pure logic:

- configuration;
- paths and atomic files;
- power/network parsers;
- policy rendering;
- agent detection and adapter documents;
- session state;
- helper protocol.

### `rucksack-cli`

User-facing binary:

- interactive flow;
- helper installation client;
- daemon mode;
- hook mode;
- adapter install/remove;
- remote-provider commands.

### `rucksack-helper`

Small root binary:

- Unix socket;
- peer credential validation plus release-build code-signature and Team ID validation;
- single lease;
- fixed `pmset` calls;
- persisted baseline;
- expiry watchdog;
- power-source notifications.

It contains no HTTP client, agent adapter, config merger, shell interpreter, or arbitrary
filesystem API.

## State locations

User state:

```text
~/Library/Application Support/Rucksack/config.toml
~/Library/Application Support/Rucksack/session.json
~/Library/Application Support/Rucksack/last-report.json
~/Library/Application Support/Rucksack/active-policy.json
~/Library/Logs/Rucksack/daemon.log
```

Root state:

```text
/var/db/rucksack/helper-state.json
/var/run/rucksack-helper.sock
/Library/PrivilegedHelperTools/io.rucksack.helper
/Library/LaunchDaemons/io.rucksack.helper.plist
```

## Session state machine

```text
inactive
   │
   ▼
preflight
   │ agent + network + battery pass
   ▼
policy_active
   │ lease acquired
   ▼
waiting_for_hotspot
   │ route + internet pass
   ▼
waiting_for_unplug
   │ battery observed + reassert + re-probe
   ▼
ready
   │ daemon heartbeats
   ▼
active ──────────────┬──────────────┬───────────────┐
   │                 │              │               │
   │ user unpacks    │ timeout      │ battery floor │ thermal
   ▼                 ▼              ▼               ▼
releasing ◄─────────────────────────────────────────┘
   │ baseline verified, report saved, policy removed
   ▼
inactive
```

Any internal inconsistency goes to `releasing`, not back to `active`.

## Helper protocol

One newline-delimited JSON request per connection.

```json
{
  "protocol": 2,
  "request_id": "uuid",
  "operation": {
    "type": "acquire",
    "lease_id": "uuid",
    "ttl_seconds": 90,
    "hard_expires_at": "2026-07-24T19:42:00Z",
    "reason": "Claude Code commute"
  }
}
```

Responses include the observed state, not only command success:

```json
{
  "protocol": 2,
  "request_id": "uuid",
  "ok": true,
  "status": {
    "lease_id": "uuid",
    "owner_uid": 501,
    "expires_at": "2026-07-24T18:42:00Z",
    "hard_expires_at": "2026-07-24T19:42:00Z",
    "previous_sleep_disabled": 0,
    "sleep_disabled": 1
  }
}
```

## Lease behavior

### Acquire

1. Validate the peer UID and, in release builds, its dynamic code signature, identifier,
   Developer ID chain, and Team ID.
2. Reject a live lease owned by another UID.
3. Validate and persist the non-renewable session deadline.
4. Read `SleepDisabled`.
5. Require the verified normal baseline `SleepDisabled=0`; refuse to coexist with another
   owner.
6. Persist the baseline and requested lease before mutation.
7. Run the fixed `pmset` command.
8. Verify `SleepDisabled=1`.

If verification fails, restore baseline and retain error state until recovery is verified.

### Renew

- require owner UID and matching lease ID;
- cap TTL to a compiled maximum;
- update persisted expiry without crossing the hard session deadline;
- verify/reassert `SleepDisabled=1`.

### Power change

- event callback wakes helper;
- helper reasserts immediately;
- helper verifies immediately and after a short debounce;
- repeated failure releases/restores.

### Release

1. retain the persisted lease until restoration succeeds;
2. restore the saved normal baseline;
3. verify;
4. delete persisted helper state;
5. return final status.

### Startup

If a persisted state file exists, restore its baseline before opening the socket. A restart
therefore fails safe even if the user daemon is still running.

## User daemon

The daemon is deliberately unprivileged. Every heartbeat:

- renew helper lease;
- read battery;
- read thermal signal;
- check hard deadline;
- check Wi-Fi/default route;
- for strict hotspot/USB sessions, compare live SSID, route interface, and gateway with the
  verified handoff route;
- make a bounded internet/provider probe;
- sample aggregate byte counters on the verified commute interface;
- update local session state;
- atomically replace the private local `last-report.json` before terminal session cleanup;
- remove temporary policy after a terminal release.

It cannot set `SleepDisabled` directly.

A different live SSID, interface, or gateway proves the strict commute route was replaced
and triggers immediate release. A missing route is treated as a temporary outage and uses
the configured reconnect grace before release.

The completed-session report retains only the latest session. Mobile-data accounting is a
start/end delta of macOS interface byte counters and is explicitly an estimate of aggregate
Mac traffic on that interface. A missing baseline or final sample, interface change, or
counter reset produces a partial/unavailable result instead of a fabricated value. No
packet capture or destination-level logging is involved. Report writes are serialized and
a stale writer that no longer owns current session state cannot replace the latest report.

## Concurrency

- helper lease state is guarded by one mutex;
- helper operations are serialized;
- event and watchdog threads enqueue work against the same state;
- user state mutations use short advisory-lock transactions, revision checks, and atomic
  temp-file + rename writes;
- `pack`, daemon release, `unpack`, and `recover` share one terminal-operation lock, so only
  one path can start or finalize a session;
- same-session report writes are idempotent, and a different session can replace the latest
  report only while it owns the current session state;
- one user session is allowed per account;
- helper allows one global lease because `SleepDisabled` is global.

## Packaging

The release-gated distribution pipeline implements:

- universal macOS binaries;
- Developer ID signing;
- hardened runtime;
- a notarized `.pkg` workflow;
- stable `rucksack-universal.pkg` and checksum asset names;
- a checksum-, signature-, and Gatekeeper-verifying `scripts/install.sh`;
- signed root helper;
- launchd plist;
- deterministic release metadata and checksums.

It has not yet completed a production-credential notarization run or the hardware gate.
A Homebrew cask remains future work. The alpha’s `helper install` command is for debug
development and review, not release authorization.
