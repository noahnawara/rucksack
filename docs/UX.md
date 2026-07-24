# CLI UX

## Design objective

The CLI should feel like a short physical ritual, not a diagnostic dashboard. The user is
already leaving and has very little spare attention.

## Information hierarchy

The default flow is a short story with explicit ownership:

1. `🎒 packing up.` or `🎒 unpacking.` names the chapter.
2. `→ rucksack is` names work the program is doing.
3. `your turn.` appears immediately before every `→` user action.
4. a wait is marked `→` and its automatic continuation is marked `↳`.
5. a measured result starts with `✓` and states only what its evidence proves.
6. optional implementation detail remains under `--verbose`.

Color, symbols, and animation may support the hierarchy but never carry meaning on their
own. Redirected output must remain just as clear as an interactive terminal.

User confirmation is always attributed with `confirmed by you`. It is never presented as a
machine measurement.

## Primary flow

```text
$ rucksack pack

🎒 packing up.

→ rucksack is checking the active agent.
✓ codex is installed.
✓ codex is active in this project.
→ rucksack is checking the native codex commute mode adapter.
✓ the native codex commute mode adapter is ready.
✓ codex pairing, native trust, and baseline phone visibility were confirmed by you during setup.
→ rucksack is arming commute mode for codex.
✓ commute mode is armed for codex with continue focus.
→ rucksack is starting codex remote control.
✓ codex remote control is running.

your turn.

→ in the open codex conversation invoke `$commute-mode rucksack-<16-hex-code>`.
→ wait for codex to acknowledge commute mode.
confirm that codex acknowledged commute mode in that exact conversation [Y/n]

✓ the exact codex task activation was observed.
→ rucksack is checking the commute connection.
→ rucksack is asking macos to join the saved hotspot Max’s iPhone.
✓ macos accepted the saved hotspot join request.
→ rucksack is waiting for Max’s iPhone to become the verified wifi route.
↳ packing will continue automatically.
✓ wifi is connected to Max’s iPhone.
✓ the default route uses en0.
→ rucksack is checking internet.
✓ internet is reachable.
→ rucksack is checking the codex endpoint.
✓ the codex endpoint is reachable.
→ rucksack is checking battery and thermal safety.
→ rucksack is securing the closed lid safety lease.
✓ the closed lid safety lease is active.

your turn.

→ unplug this mac while the lid is open.
→ rucksack is waiting for battery power.
↳ packing will continue automatically.
✓ this mac is running on battery at 78 percent.
→ rucksack is rearming the safety lease after unplugging.
✓ the closed lid safety lease survived unplugging.
→ rucksack is checking the connection after unplugging.
✓ internet after unplugging is reachable.
✓ the network and the codex endpoint survived unplugging.
→ rucksack is starting mobile data accounting on en0.
✓ mobile data accounting is active on en0.
→ rucksack is checking the final safety limits.
✓ battery is 78 percent. rucksack warns at 20 percent and restores normal sleep at 15 percent.
✓ thermal pressure is nominal.
✓ the session ends at 19.42.
→ rucksack is starting the safety watcher.
→ rucksack is waiting for the safety watcher to report its first heartbeat.
↳ packing will finish automatically.
✓ the safety watcher is running.

🎒 packed.

your turn.

→ lock this mac.
→ close the lid and go.
✓ rucksack will restore normal sleep automatically.
```

The user should never be asked to remember “start Amphetamine before unplugging” or
“run pmset again.” The correct sequencing is the interface.

## First run

```text
$ rucksack setup

Rucksack Setup
Four things make the handoff reliable.

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

4. Remote Control
Codex: if needed, run `rucksack pair codex` and finish pairing in ChatGPT.
Codex: open `/hooks`, review the marked Rucksack entries, and trust them.
Codex: confirm that ChatGPT on your phone can see a remote session.
Confirm that Codex pairing, native adapter trust, and baseline phone visibility are complete [y/N]
✓ Codex pairing, native trust, and baseline phone visibility were confirmed by you.

Defaults
  Commute deadline: 24 hours
  Warn: 20% battery
  Sleep: 15% battery
  Thermal release: serious

Setup complete.
Run `rucksack pack` the next time you walk out.
```

For a published release, `scripts/install.sh` verifies the stable
`rucksack-universal.pkg` asset and invokes the system installer after administrator
authentication. The package installs the helper, so `rucksack setup` reports it reachable
instead of using the debug-only helper installer.

## Connection modes

With `--hotspot "Max’s iPhone"`, Rucksack first asks macOS to join that saved Wi-Fi
network without asking an extra confirmation. It never accepts a hotspot password because
command arguments are visible to other local processes. If the saved-network request
fails, or the phone is advertised only through Apple Instant Hotspot, Rucksack prints one
`your turn` block that tells the user to select it from the Wi-Fi menu. It then waits for
the configured route and continues automatically. If the saved-network request succeeds
but macOS privacy-hides the connected name, Rucksack asks for one exact Wi-Fi-menu
confirmation. That explicit confirmation is sufficient evidence.
`--allow-unverified-ssid` permits a redacted configured SSID after a successful exact
saved-network join request. `--yes` cannot supply interactive evidence.

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
conversation, the stored baseline phone onboarding is current, and the fresh token binds
that exact provider session. With neither automatic startup nor a running conversation,
the handoff stops.

## Existing Claude Code session

Rucksack cannot safely inject `/remote-control` into a live terminal. The UI says so
without presenting it as an error:

```text
rucksack is checking claude code remote control support.
claude code remote control is available.
a claude code conversation is running.
claude code pairing, native trust, and baseline phone visibility were confirmed by you during setup.

your turn.

in the active claude code conversation run `/remote-control`.
wait until `/rc active` appears.
invoke `/commute-mode rucksack-<16-hex-code>` in that exact conversation.
confirm that claude code acknowledged commute mode in that exact conversation.

the exact claude code task activation was observed.
```

## Cursor

Cursor Remote Control currently requires a UI action:

```text
your turn.

in the open cursor conversation invoke `/commute-mode rucksack-<16-hex-code>`.
wait for cursor to acknowledge commute mode.
in cursor open agents and then remote control.
confirm that cursor acknowledged commute mode in that exact conversation.

the exact cursor task activation was observed.
```

Do not say “Remote Control verified” until Cursor exposes a stable machine-readable API.

## Permission inheritance

Commute Mode inherits the active agent session's permission, approval, and sandbox
configuration exactly. Rucksack does not enable, disable, tighten, bypass, approve, or deny
provider permissions. `PermissionRequest` hooks are passive lifecycle signals only and
return no decision.

## Pairing Codex

```text
$ rucksack pair codex

Codex Remote Control

Pairing code
  J7K9-M2Q4

Open ChatGPT on your phone and enter the code.
Expires at 2026-07-24T18:31:00Z
Confirm that Codex pairing completed and your phone can see a remote session [y/N]
✓ Pairing and baseline phone visibility were confirmed by you.
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

## Unpack

```text
$ rucksack unpack

🎒 unpacking.

rucksack is restoring normal sleep.
normal sleep is restored.
rucksack is stopping the safety watcher.
rucksack is waiting for the safety watcher to stop.
unpacking will continue automatically.
the safety watcher has stopped.
rucksack is removing commute mode.
commute mode is removed.

🎒 unpacked.

🎒 trip report.

codex worked in ~/work/atlas.
the rucksack was packed for 42m 18s.
the session ended at 24 july 2026 at 19.42.
the session ended by unpack because user unpacked.
battery moved from 78 percent to 61 percent.
estimated mobile data was 184.2 MB.
151.7 MB was downloaded.
32.5 MB was uploaded.
this counts all traffic on en0.
this is not agent only usage or carrier billing.
```

Automatic release writes the same report before the active session ends. `status` describes
the live session; `report` reads the most recent completed session:

```text
$ rucksack report

🎒 trip report.

codex worked in ~/work/atlas.
the rucksack was packed for 42m 18s.
the session ended at 24 july 2026 at 19.42.
the session ended by automatic release because commute route moved to ordinary wifi.
battery moved from 78 percent to 61 percent.
estimated mobile data was 184.2 MB.
151.7 MB was downloaded.
32.5 MB was uploaded.
this counts all traffic on en0.
this is not agent only usage or carrier billing.
```

The estimate covers all Mac traffic observed on the verified commute interface during the
measurement window. Missing samples or counter resets are reported as partial or
unavailable, never fabricated as zero. `rucksack --json report` returns the typed report.

## Recovery

```text
$ rucksack recover

🎒 recovering.

your turn.

allow rucksack to restore normal sleep and clear interrupted state.
rucksack is restoring normal sleep.
normal sleep is restored.
temporary policy and stale state are cleared.

🎒 recovered.

this mac will sleep normally.
```

## Copy rules

- State facts, not implementation.
- Use `this mac`, not `the host`.
- Use “normal sleep,” not “baseline power configuration.”
- Begin Rucksack work with `rucksack is`.
- Put `your turn` immediately before every user action.
- Name the condition and automatic continuation before a bounded wait.
- Keep measured results literal and deterministic.
- Never ask an agent model to generate handoff copy.
- Use lowercase for authored human copy. Preserve paths, SSIDs, commands, and provider values
  exactly.
- Use periods in story copy. Terminal controls and exact technical values are exempt.
- Name the exact consequence of a failure.
- Never say “probably.”
- Never blame the user for the wrong sequence; the tool owns sequencing.
- Default to a single recommendation.
- Hide technical remediation behind `Show details`/`--verbose`.
- “Packed” is a reserved word and means every mandatory invariant passed.

## Non-interactive mode

CI and advanced users can use:

```text
rucksack pack \
  --agent codex \
  --hotspot "Max’s iPhone" \
  --for 75m \
  --focus finish \
  --yes \
  --json
```

`--yes` skips ordinary measured-state confirmations but cannot fabricate UI-only setup
evidence. During packing, non-interactive mode waits up to two minutes for the native hook
to bind the fresh task activation. The explicit
`--allow-unverified-remote` exception accepts missing stored phone-onboarding and/or
provider-endpoint evidence; it never bypasses the exact tokenized task binding.
`--force` is not a general escape hatch. Individual unsafe exceptions must be named and
recorded.
