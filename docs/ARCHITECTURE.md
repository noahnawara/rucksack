# Architecture

## Components

```text
┌─────────────────────────────────────────────────────────────┐
│ rucksack CLI                                                │
│ pack · status · unpack · pair · star · helper               │
└───────────────┬─────────────────────────────────────────────┘
                │ typed JSON over Unix socket
┌───────────────▼─────────────────────────────────────────────┐
│ rucksack-helper (root LaunchDaemon)                         │
│ peer-UID auth · lease · pmset · power events · watchdog     │
└───────────────▲─────────────────────────────────────────────┘
                │ renew · release
┌───────────────┴─────────────────────────────────────────────┐
│ rucksack daemon (user)                                      │
│ heartbeats · battery · thermal · route · internet probe     │
└───────────────┬─────────────────────────────────────────────┘
                │ session.json under an advisory lock
┌───────────────▼─────────────────────────────────────────────┐
│ session state, read by status and unpack                    │
└─────────────────────────────────────────────────────────────┘
```

`pack` also spawns Codex Remote Control as a child it never waits for, and writes one
marker-guarded `SKILL.md` into each agent's skills directory that already exists. There are
no hooks and no editor rules.

## Crates

### `rucksack-core`

No privilege. Shared types and pure logic:

- `config` — the on-disk configuration, which ignores keys older releases wrote;
- `paths` and `files` — where state lives, plus atomic writes and advisory locks;
- `power` and `network` — `pmset`, battery, thermal, Wi-Fi, route, and probe parsers;
- `state` — the host session record and its phases;
- `protocol` — the helper request, response, and status types;
- `skill` — the marker-guarded agent skill file;
- `codex` — finding Codex and building its Remote Control arguments;
- `system` — bounded command execution, `which`, process listing, and the current UID.

There is no agent module and no adapter machinery.

### `rucksack-cli`

User-facing binary:

- `cli` and `app` — six visible verbs plus a hidden `daemon`, and their dispatch;
- `flow` — `pack`, `status`, and `unpack`;
- `daemon` — the safety watcher;
- `thermal` — `ProcessInfo.thermalState` over a small Objective-C FFI, the only thermal source
  Apple silicon actually answers, kept here so the root helper samples no temperature;
- `install` — installing and removing the root helper through one `sudo` prompt;
- `helper_client` — one request and one response per socket connection;
- `output` — `step`, `done`, `warn`, and a `detail` only `--verbose` prints;
- `star` — the GitHub star, through the `gh` CLI the user is already signed in to, or by opening the
  page. The first completed `unpack` mentions it once, ever; the asking is left to the agent, which
  is the only party in a conversation and can put a real question in front of the user.

### `rucksack-helper`

Small root binary:

- `server` — the Unix socket, peer authentication, connection limits, and the two
  background threads;
- `lease` — one lease, its persisted baseline, the fixed `pmset` calls, and expiry;
- `power_events` — IOKit power-source notifications on a run loop.

It contains no HTTP client, config merger, shell interpreter, or arbitrary filesystem API.

### Privileged boundary

The helper authenticates a caller by peer UID through `getpeereid`, on a `root:admin` `0660`
socket, and checks lease ownership on every operation. It also verifies the calling binary's
Apple code signature — identifier, Developer ID chain, and Team ID — but only when it was
compiled with `RUCKSACK_TEAM_ID`, which is set for signed release packages. A build from
source has no team ID and authenticates by UID alone, which is what makes
`cargo build --release` usable; such a helper says so on startup.

## State locations

User state:

```text
~/Library/Application Support/Rucksack/config.toml
~/Library/Application Support/Rucksack/session.json
~/Library/Application Support/Rucksack/session.lock
~/Library/Application Support/Rucksack/session.terminal.lock
~/Library/Application Support/Rucksack/asked-about-star
~/Library/Logs/Rucksack/daemon.log
~/Library/Logs/Rucksack/remote-control.log
```

The data directory is created `0700` and everything rucksack writes into it is `0600`. No
session file is the resting state: `unpack` deletes `session.json`.

Root state:

```text
/var/db/rucksack/helper-state.json
/var/run/rucksack-helper.sock
/var/log/rucksack-helper.log
/Library/PrivilegedHelperTools/io.rucksack.helper
/Library/LaunchDaemons/io.rucksack.helper.plist
```

## Session state machine

```text
ready
   │ the watcher's first heartbeat
   ▼
active
   │ unpack, the time limit, the battery floor, real heat, or a battery
   │ gauge that went silent on battery
   ▼
released
   │ unpack clears the record
   ▼
no session
```

A lease belongs to this Mac, so nothing about a conversation appears in the record and
nothing about a conversation ends it. Losing the network and an agent finishing its work are
recorded and neither releases the lease.

When the helper stops answering a renewal, the watcher exits with that error and leaves the
record `active`; the helper's own TTL restores sleep, and the next `unpack` clears the
record.

## Helper protocol

One newline-delimited JSON request per connection, at most 32 connections at once, 256 KiB
per request, and a ten-second read and write timeout. The operations are `acquire`, `renew`,
`reassert`, `release`, `recover`, and `status`.

```json
{
  "protocol": 2,
  "request_id": "uuid",
  "operation": {
    "type": "acquire",
    "lease_id": "uuid",
    "ttl_seconds": 90,
    "hard_expires_at": "2026-07-24T19:42:00Z",
    "reason": "rucksack"
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
    "active": true,
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

1. Validate the peer UID and, where a Team ID was compiled in, the peer's code signature.
2. Require a TTL of 30 to 300 seconds and a reason of no more than 256 bytes.
3. Reject a live lease; the same owner asking again for the same lease ID renews instead,
   and cannot move its hard deadline.
4. Require a hard deadline that is in the future and no more than 24 hours away.
5. Read `SleepDisabled` and require the normal baseline `0`, so there is an unambiguous
   rollback target and no coexistence with another owner.
6. Persist the baseline and the lease before mutating anything.
7. Run the fixed `pmset` command.
8. Verify `SleepDisabled=1`.

If verification fails, the helper restores the baseline. When that works it drops the lease
and deletes its state; when it does not, it marks the lease expired so the watchdog keeps
trying, and says both things failed.

### Renew

- require the owner UID and a matching lease ID;
- release, and refuse the renewal, when either deadline has already passed;
- cap the TTL at the compiled maximum and clamp it to the hard deadline;
- reassert and verify `SleepDisabled=1` when it is no longer `1`.

### Watchdog

Every five seconds: release when the lease TTL or the hard deadline has passed, and reassert
when `SleepDisabled` is not `1`.

### Power change

- an IOKit power-source notification wakes the helper;
- it writes the setting immediately, again after a short debounce, and then verifies;
- any failure restores the baseline and reports that it did.

### Release

1. keep the persisted lease until restoration succeeds;
2. restore the saved normal baseline;
3. verify;
4. delete persisted helper state;
5. return the final status.

`recover` is the same path without a lease ID, for the owner UID or root.

### Startup

If a persisted state file exists, restore its baseline before opening the socket; a state
file that will not parse or does not validate is restored to `SleepDisabled=0` and deleted.
A restart therefore fails safe even if the user daemon is still running.

## User daemon

The daemon is deliberately unprivileged. Every heartbeat:

- check the hard deadline, the battery floor, thermal pressure, and whether the battery is
  readable at all, and release when one of those says to;
- renew the helper lease;
- read the battery, the default route, and the Wi-Fi name;
- make a bounded internet probe;
- update the session record, including whether the network came or went.

It cannot set `SleepDisabled` directly.

The probe and the route only feed `status`. Three consecutive failures to read the battery
while on battery end the session, because a Mac flying blind on battery is the one case
where staying awake is unsafe; a silent gauge on AC power is ordinary and means nothing.

## Concurrency

- helper lease state is guarded by one mutex;
- helper operations are serialized;
- the watchdog and power-event threads take the same lock as request threads;
- user state changes are read-modify-write under one advisory lock, written to a temporary
  file and renamed into place, and ignored when the session on disk is a different one;
- `pack`, daemon release, and `unpack` share one terminal-operation lock, so only one path
  can start or finish a session;
- one user session is allowed per account;
- the helper allows one global lease because `SleepDisabled` is global.

## Packaging

The release-gated distribution pipeline implements:

- universal macOS binaries;
- Developer ID signing;
- hardened runtime;
- a notarized and stapled `.pkg` workflow;
- stable `rucksack-universal.pkg` and checksum asset names;
- a checksum-, signature-, and Gatekeeper-verifying `scripts/install.sh`;
- signed root helper;
- launchd plist;
- deterministic release metadata and checksums.

It has not yet completed a production-credential notarization run or the hardware gate. A
Homebrew cask remains future work.
