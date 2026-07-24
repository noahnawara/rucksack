# CLI UX

## Design objective

The CLI should feel like a short physical ritual, not a diagnostic dashboard. The user is
already leaving and has very little spare attention.

## Information hierarchy

Every screen uses four levels:

1. **Product title and current goal**
2. **Section**: Agent, Connection, Power, Safety
3. **Measured status or one required action**
4. **Optional detail under `--verbose`**

Symbols:

- `✓` measured and passed;
- `→` user action required;
- `…` checking;
- `!` warning with a safe default;
- `×` blocking failure.

Do not use a green check for user confirmation. Use `✓ Confirmed by you` so the evidence
source remains honest.

## Primary flow

```text
$ rucksack leave

Rucksack
Preparing this Mac for the walk home

Agent
✓ Codex is running in ~/work/atlas
✓ Remote Control daemon is available
✓ Commute policy loaded

Connection
→ Connect “Max’s iPhone”
✓ Wi-Fi: Max’s iPhone
✓ Internet: reachable
✓ Codex remote endpoint: reachable

Power
→ Unplug this Mac while the lid is open
… Waiting for battery power
✓ Running on battery
✓ Closed-lid lease re-armed
✓ Hotspot and remote route survived the transition

Safety
✓ Battery 78% · warn at 20% · sleep at 15%
✓ Thermal pressure normal
✓ Session ends at 19:42

Ready.
Lock your Mac, close the lid, and go.
Normal sleep will be restored automatically.
```

The user should never be asked to remember “start Amphetamine before unplugging” or
“run pmset again.” The correct sequencing is the interface.

## First run

```text
$ rucksack setup

Rucksack Setup
Three things make the handoff reliable.

1. Hotspot
→ Connect the hotspot you normally use
✓ Found “Max’s iPhone”
   Save this name? [Y/n]

   Or choose the wired path explicitly:
   $ rucksack setup --usb

2. Power helper
   Lets Rucksack hold a time-limited closed-lid lease.
   macOS will ask for an administrator password once.
→ Install helper? [Y/n]

3. Coding agents
✓ Codex found
✓ Claude Code found
✓ Cursor found
→ Install reversible Commute Mode adapters? [Y/n]

Defaults
  Commute: 75 minutes
  Warn: 20% battery
  Sleep: 15% battery
  Thermal release: serious

Setup complete.
Run `rucksack leave` the next time you walk out.
```

For a published release, `scripts/install.sh` verifies the stable
`rucksack-universal.pkg` asset and invokes the system installer after administrator
authentication. The package installs the helper, so `rucksack setup` reports it reachable
instead of using the debug-only helper installer.

## Connection modes

With `--hotspot "Max’s iPhone"`, Rucksack first asks macOS to join that saved Wi-Fi
network. It never accepts a hotspot password because command arguments are visible to
other local processes. If the phone is advertised only through Apple Instant Hotspot,
Rucksack gives one bounded prompt to select it from the Wi-Fi menu and then verifies the
route and internet path. `--allow-unverified-ssid` permits a privacy-redacted configured
SSID only after a successful exact join request or interactive confirmation that the
Wi-Fi menu shows that hotspot. `--yes` cannot supply this evidence.

With `--usb`, Rucksack waits for the `iPhone USB` network device to become the default
route. A connected charging cable is not sufficient evidence: Personal Hotspot must be
enabled on the phone, and ordinary Wi-Fi must not remain the route being tested.

Both strict modes bind the verified route interface and gateway for the session. If a
different live SSID, interface, or gateway appears, Rucksack restores normal sleep
immediately. If the route simply disappears, status enters a temporary-offline state and
the configured reconnect grace begins.

Ordinary Wi-Fi auto-join remains a macOS responsibility in 0.1. Rucksack observes the
result and releases immediately; it does not guess among the user's preferred networks.
An explicitly configured return-network action is deferred to 0.2.

## Agent ambiguity

```text
Agent
Two live agents match this folder.

1. Claude Code — terminal
2. Cursor — desktop

Select the session you will control from your phone [1]:
```

The choice should show product names and surfaces, not process IDs.

## Codex command capability

Rucksack first attempts `codex remote-control start`. Some bundled Codex installations do
not expose that standalone command even while an existing provider-hosted conversation is
available. In that case, Rucksack continues only when it detects a running Codex
conversation and the user confirms the exact session on the phone. With neither automatic
startup nor a running conversation, the handoff stops.

## Existing Claude Code session

Rucksack cannot safely inject `/remote-control` into a live terminal. The UI says so
without presenting it as an error:

```text
Agent
✓ Claude Code is running
→ In Claude Code, run `/remote-control`
  Return here when the `/rc active` indicator appears.
```

## Cursor

Cursor Remote Control currently requires a UI action:

```text
Agent
✓ Cursor is running
→ In the open Cursor conversation, invoke `/commute-mode` once
→ In Cursor, open Agents → Remote Control
  Confirm the phone can see this agent, then return here.
✓ Confirmed by you
✓ Temporary project rule, command, and telemetry hooks loaded
```

Do not say “Remote Control verified” until Cursor exposes a stable machine-readable API.

## Pairing Codex

```text
$ rucksack pair codex

Codex Remote Control

Pairing code
  J7K9-M2Q4

Open ChatGPT on your phone and enter the code.
Expires at 2026-07-24T18:31:00Z
```

Machine JSON is available with `--json`.

## Status

Default status is brief:

```text
$ rucksack status

Commute Mode is active
✓ Claude Code · ~/work/atlas
✓ Online through Max’s iPhone
Battery 61% · 38 minutes remaining
State: WaitingForApproval
```

`rucksack status --verbose` adds the last event and helper record. `rucksack status --full`
prints the complete session/helper view as JSON.

## Arrival

```text
$ rucksack arrive

Rucksack
✓ Normal sleep restored
✓ Commute policy removed
✓ Watcher stopped

Arrived.
```

## Recovery

```text
$ rucksack recover

Recovery
✓ Normal sleep restored
✓ Temporary policy and stale state cleared

This Mac will sleep normally.
```

## Copy rules

- State facts, not implementation.
- Use “this Mac,” not “the host.”
- Use “normal sleep,” not “baseline power configuration.”
- Put the physical action first.
- Name the exact consequence of a failure.
- Never say “probably.”
- Never blame the user for the wrong sequence; the tool owns sequencing.
- Default to a single recommendation.
- Hide technical remediation behind `Show details`/`--verbose`.
- “Ready” is a reserved word and means every mandatory invariant passed.

## Non-interactive mode

CI and advanced users can use:

```text
rucksack leave \
  --agent codex \
  --hotspot "Max’s iPhone" \
  --for 75m \
  --focus finish \
  --yes \
  --json
```

`--yes` skips ordinary setup confirmations but cannot skip physical state checks,
privacy-redacted SSID confirmation, or a UI-only phone-visibility check. The explicit
`--allow-unverified-remote` exception accepts missing provider-endpoint evidence and/or
phone-visibility evidence; that risk is recorded. `--force` is not a general escape
hatch. Individual unsafe exceptions must be named and recorded.
