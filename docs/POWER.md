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

Power assertions prevent idle sleep. Closing a laptop lid is a forced sleep condition, so
`caffeinate` is not the closed-lid primitive. rucksack does not use it, and takes no power
assertion of its own.

## What Amphetamine Power Protect actually does

Power Protect does not simulate AC. It gives Amphetamine permission to run the narrow
commands:

```text
/usr/bin/pmset -a disablesleep 1
/usr/bin/pmset -a disablesleep 0
```

Amphetamine re-applies the setting after external-power changes on Apple-silicon Macs.
rucksack adopts the mechanism but changes the lifecycle:

- no indefinite global toggle;
- exact baseline preservation;
- root-owned lease;
- heartbeat expiry;
- power-source reassertion;
- battery/thermal release.

## Why rucksack does not fake AC

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

rucksack changes only the sleep decision it needs to change.

## What `pack` does

In order, from `pack_inner` in `crates/rucksack-cli/src/flow.rs`:

1. Install the rucksack skill, best-effort and never fatal. First, because the first `pack` is
   the one that needs it most: the skill is what tells an agent how to behave while `pack` is
   waiting, and writing it last gave it to everyone except the person who had not packed before.
2. Refuse if rucksack already holds a live lease — one whose watcher is still running and still
   writing. A session whose watcher died does not block a new `pack`; it says so and continues.
3. Merge the command-line options over the saved configuration and validate the result.
4. Refuse if `SleepDisabled` is already 1.
5. Read the battery: refuse at or below the sleep floor, warn at or below the warning
   threshold. A gauge that reports nothing is silence, not a refusal.
6. Read thermal pressure: refuse on anything that would end a lease, so `pack` cannot accept
   a Mac the watcher would release on its first heartbeat. An unreported level is silence.
7. Install the power helper if it is absent. This is the one macOS password prompt.
8. Reach a commute network.
9. Remember that network as the hotspot if none was saved yet.
10. Acquire the helper lease.
11. Start Codex Remote Control as a fire-and-forget child. Failure warns; only
    `--require-remote` makes it fatal.
12. Write the session state, spawn the watcher, and wait for its first heartbeat.
13. Print how long the Mac stays awake, then “Packed. Close the lid and go.”

If any step after the lease fails, `pack` rolls back: it stops the watcher, releases the
lease, and clears the session state.

There is no unplug step. Packing while plugged in is allowed, and a later unplug is handled
by the helper's own power-source observer.

rucksack refuses to start when `SleepDisabled` is already 1, because the recorded baseline
is what the helper restores on release. Whatever already switched sleep off — Amphetamine's
closed-display mode, an earlier crash — leaves rucksack no unambiguous state to hand back.

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
- acquisition requires `previous_sleep_disabled=0` and refuses while a lease is held;
- the owner or root can renew, re-assert, release, or recover an active lease;
- renewable expiry cannot cross `hard_expires_at`, which is capped at 24 hours;
- either deadline elapsing restores `previous_sleep_disabled`;
- helper restart restores stale state before accepting a new lease;
- persisted state the helper cannot parse or validate restores `SleepDisabled=0` and is
  deleted;
- a restore that fails marks the lease expired so the watchdog retries it.

Heartbeat freshness is represented by the renewable `expires_at`; it is not a separate
persisted timestamp.

## Power-source observation

The macOS helper uses `IOPSNotificationCreateRunLoopSource` to receive power-source
changes. On each event while a lease is active:

1. re-apply `pmset -a disablesleep 1` immediately;
2. re-apply it again after a 250 ms debounce;
3. verify the setting with `pmset -g`.

If any of that fails, the helper restores the recorded baseline: a helper that cannot prove
the override is active fails safe to ordinary sleep.

A separate watchdog thread ticks every five seconds. It releases the lease once either
deadline has elapsed, and re-asserts `SleepDisabled` whenever the setting is no longer 1.

## Why a lease ends

A held lease ends for six reasons and no others: `rucksack unpack`, the session's time
limit, the battery floor, serious or critical thermal pressure or reported throttling, three
consecutive failed battery reads while on battery, and the helper heartbeat failing — after
which the helper's own TTL restores sleep. Losing the network is not one of them, and
neither is an agent finishing its work.

## Battery

Defaults:

- warning: 20%;
- sleep release: 10%.

There is no minimum battery level to start; `pack` refuses only at or below the release
floor, because such a Mac would sleep the moment the lid closed. At the floor the watcher
restores normal sleep. It does not attempt to finish “one last build.”

## Time remaining

The lease clock and the battery are separate limits, and on a commute the battery is nearly
always the smaller: a 24 hour lease on a Mac with four hours of charge never had 24 hours to
give. `pack` and `status` both report whichever runs out first, and mark a projected figure
as an estimate so it does not read like arithmetic on a known deadline.

`pack` answers before any watcher exists, so it has no drain of its own to project from and
borrows the same scaled macOS estimate described below. Only the lease branch quotes a wall
clock deadline; the battery branch says "about", because that is all it is.

The watcher projects from drain it has actually seen. Percent readings are whole numbers, so
at a realistic rate the gauge only moves every few minutes; differencing every heartbeat
would measure quantisation rather than drain. Only drops are recorded, and two are needed
before anything is claimed, so both ends of the measured span are real transitions.

Three silences invalidate the projection rather than stretch it. A gap longer than three
heartbeats means the Mac slept, and sleep advances the wall clock while the battery barely
moves. Charging clears it, because there is no drain left to project. A heartbeat that could
not read the gauge is a tunnel rather than a discontinuity and carries the window forward.

Measuring a rate takes three readings — the first is a baseline and makes no drop — so for
the opening minutes of a session there is nothing of this Mac's own to report. macOS has an
estimate by then, and `pmset` is already being read every heartbeat, so that figure is used
until rucksack has one of its own. It is scaled first: macOS measures to empty and rucksack
stops at the floor, so the session is always shorter than the battery, and reporting the
borrowed number unscaled would over-promise.

A measurement of this Mac's actual workload always outranks a general-purpose estimate, so
the borrowed figure is dropped the moment two drops exist. Neither is dressed up as the
other; both are estimates, and both are marked `~`.

A session whose heartbeat has gone quiet stops claiming a battery figure nobody is still
taking.

## Winding down

Ten minutes before whichever limit binds, the watcher records the wind-down on the session and
`status` leads with it. The end is otherwise silent: every running task stops the moment the Mac
sleeps, mid-step, and whatever lived only in a conversation goes with it. The warning is what
gives an agent time to write its state to disk instead.

It is not a release condition. The list above is still the whole of what ends a lease, and a
wind-down that reaches the floor ends the session there in the ordinary way.

The threshold is asymmetric: set at ten minutes, cleared only past twenty. A projection wobbling
either side of the line must not retract a deadline an agent has already begun packing up for,
while a Mac that has been plugged in genuinely is not ending soon — both battery sources go quiet
on mains power, the lease clock takes over, and the warning is called off.

rucksack cannot interrupt a running agent; no such channel exists for Codex, Claude Code, or
Cursor. The warning lands where an agent already looks, and the installed skill says what to do
about it.

## Thermal pressure

The unprivileged watcher reads two sources, because neither covers every Mac on its own.

`ProcessInfo.thermalState` is the primary signal, read over a small Objective-C FFI in
`crates/rucksack-cli/src/thermal.rs`. It is public, documented, needs no privilege, and is
the only one of the two that moves on Apple silicon.

Bounded `pmset -g therm` output is still parsed for `CPU_Speed_Limit` and
`CPU_Scheduler_Limit`. Those are Intel-era counters. Apple silicon never populates them, so
on that hardware `pmset -g therm` prints three "has been recorded" notes at every
temperature and reports no level at all. Absent counters therefore parse to `Unknown`, an
unread sensor rather than a healthy one; reading them as nominal would state a level that
was never measured, and would have been the only level the watcher ever saw.

Behavior, taking whichever source reports the worse state:

- nominal: continue;
- fair without throttling: continue;
- serious or critical: release the lease;
- any reported CPU speed or scheduler throttling: release the lease.

An unreported thermal level is not a release reason: macOS declining to say anything is
silence, not heat. That now means both sources declining to answer, which on a working Mac
does not happen.

The FFI lives in the watcher rather than the helper, so the privileged process links no
Foundation and samples no temperature.

High CPU utilization alone is not a thermal signal. rucksack allows task-required work and
uses macOS thermal pressure and throttling telemetry as the stop condition.

## Display and screen lock

rucksack allows the display to sleep. It does not use a display-awake assertion.

rucksack does not lock the screen either. Synthesizing the lock shortcut would require
accessibility automation and another privileged surface, so locking before the lid closes
is left to the user.

## Real external power

For long or heavy tasks, the correct answer is a USB-C PD power bank or an always-on
desktop/devbox. Software should not pretend those thermal and energy constraints do not
exist.
