# Power, clamshell mode, and the hotspot

## The observed symptom

A MacBook remains reachable while closed and connected to external power. The moment the
USB-C power connection is removed, the iPhone hotspot disappears.

That looks like a hotspot policy, but in the common case it is a sleep transition:

```text
power removed
  → closed-display policy reevaluated
  → Mac sleeps
  → CPU and agent stop
  → Wi-Fi powers down
  → hotspot loses its client
```

The network loss is downstream of sleep.

## Why `caffeinate` is not enough

Power assertions prevent idle sleep. Closing a laptop lid is a forced sleep condition.
`caffeinate` is useful defense-in-depth while the lid is open, but it is not the
closed-lid primitive.

## What Amphetamine Power Protect actually does

Power Protect does not simulate AC. It gives Amphetamine permission to run the narrow
commands:

```text
/usr/bin/pmset -a disablesleep 1
/usr/bin/pmset -a disablesleep 0
```

Amphetamine re-applies the setting after external-power changes on Apple-silicon Macs.
Rucksack adopts the mechanism but changes the lifecycle:

- no indefinite global toggle;
- exact baseline preservation;
- root-owned lease;
- heartbeat expiry;
- power-source reassertion;
- battery/thermal release;
- explicit post-unplug validation.

## Why Rucksack does not fake AC

The system power source is derived from hardware and exposed by macOS power-management
services. There is no supported API for an application to declare “pretend the charger
is connected.”

Attempting to spoof AC would be the wrong abstraction for four reasons:

1. It would require private or kernel-level behavior that may break under SIP, updates, or
   Apple-silicon hardware changes.
2. Other system components would receive a lie and might make inappropriate charging,
   performance, or thermal decisions.
3. It would be harder to recover from than a single documented power-management flag.
4. A real USB-C PD battery is available when actual external power is required.

Rucksack changes only the sleep decision it needs to change.

## Required handoff order

The product should enforce this order:

1. Keep the lid open.
2. Start or verify the agent remote.
3. Connect the hotspot.
4. Verify the SSID when available. With the explicit redacted-SSID exception, require
   either successful exact join-request evidence or interactive Wi-Fi-menu confirmation.
5. Verify a default route and real internet.
6. Acquire the closed-lid lease.
7. Remove external power while the lid is still open.
8. Observe battery power.
9. Re-assert `SleepDisabled`.
10. Re-check the route, internet, and provider endpoint.
11. Start heartbeat/safety monitoring.
12. Say “Packed.”
13. User locks and closes the Mac.

Unplugging with the lid open is a deliberate race-elimination strategy. The power-change
observer still exists for later transitions, but the product does not depend on winning a
sub-second race against sleep for its primary path.

For a configured hotspot or USB tether, Rucksack persists the verified route interface and
gateway. A different live SSID, interface, or gateway means the commute route was replaced
and releases the lease immediately. A missing route may be a transient mobile outage, so it
uses the configured reconnect grace before release.

Rucksack also inspects active `pmset` assertion owners before starting. Amphetamine and
`caffeinate` are never stopped or modified, but an active assertion from either utility
blocks readiness because it would make sleep ownership and cleanup results ambiguous.
Users do not need another keep-awake utility while Rucksack owns its time-limited lease.

## Lease invariants

A valid closed-lid lease has:

```text
lease_id
owner_uid
created_at
expires_at
hard_expires_at
previous_sleep_disabled
reason
last_reasserted_at
```

Invariants:

- only one global lease exists;
- acquisition requires `previous_sleep_disabled=0` and refuses another owner;
- the owner or root can renew, re-assert, release, or recover an active lease;
- renewable expiry cannot cross `hard_expires_at`;
- expiry restores `previous_sleep_disabled=0`;
- helper restart restores stale state before accepting a new lease;
- `SleepDisabled=1` with no valid lease is an error;
- a failed restore leaves recovery state intact and retries.

Heartbeat freshness is represented by the renewable `expires_at`; it is not a separate
persisted timestamp.

## Power-source observation

The macOS helper uses `IOPSNotificationCreateRunLoopSource` to receive power-source
changes. On each event while a lease is active:

1. immediately re-apply `pmset -a disablesleep 1`;
2. verify `pmset -g`;
3. re-check after a short debounce;
4. record success or release on repeated failure.

A one-second polling fallback is acceptable for diagnostics, not for the primary safety
claim.

## Battery

Defaults:

- minimum to start: 35%;
- warning: 20%;
- sleep release: 15%.

At the floor, Rucksack restores normal sleep. It does not attempt to finish “one last
build.”

## Thermal pressure

The current unprivileged daemon parses bounded `pmset -g therm` output as a conservative
signal. A later field beta should read public `ProcessInfo.thermalState` from the
unprivileged process through Rust/Objective-C FFI; thermal sampling does not belong in the
root helper.

Default behavior:

- nominal: continue;
- fair without throttling: continue;
- serious or critical: release the lease;
- any reported CPU speed or scheduler throttling: release the lease.

High CPU utilization alone is not a thermal signal. Rucksack allows task-required work and
uses macOS thermal pressure and throttling telemetry as the stop condition.

## Display and screen lock

Rucksack allows the display to sleep. It does not use a display-awake assertion.

The user should lock the Mac before closing it. Rucksack does not synthesize the lock
keyboard shortcut because that would require accessibility automation and creates another
privileged surface.

## Real external power

For long or heavy tasks, the correct answer is a USB-C PD power bank or an always-on
desktop/devbox. Software should not pretend those thermal and energy constraints do not
exist.
