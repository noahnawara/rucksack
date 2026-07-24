# User stories and acceptance criteria

## Epic A: pack a live agent

### A1. One-command handoff

As a developer with a live coding agent, I run `rucksack pack` and receive a guided
handoff without needing to remember power-management commands.

Acceptance criteria:

- the current project is inferred from the working directory;
- active Codex, Claude Code, and Cursor processes are detected;
- one agent is selected automatically when unambiguous;
- ambiguity is shown as a short choice, not a configuration error;
- no root prompt appears after the one-time helper setup;
- “Packed” is impossible before post-unplug verification succeeds.

### A2. Connect the hotspot at the right moment

As a developer still at my desk, I am told exactly when to connect the hotspot and when to
remove external power.

Acceptance criteria:

- rucksack observes the active Wi-Fi interface;
- configured SSID is verified when macOS exposes it;
- a configured privacy-redacted SSID is accepted after the user explicitly confirms the
  Wi-Fi menu, or after an exact saved-network join request succeeds together with
  `--allow-unverified-ssid`;
- `--yes` alone cannot confirm a privacy-redacted configured SSID;
- default route and real HTTP connectivity are checked;
- rucksack asks the user to unplug while the lid remains open;
- the route and provider endpoint are rechecked after battery power is observed;
- configured hotspot/USB sessions retain the verified route interface and gateway;
- a different live SSID, interface, or gateway releases immediately, while missing-route
  outages use reconnect grace.

### A3. Close the lid with confidence

As a developer, I see a final readiness summary with the lease expiry, battery floor, and
thermal release conditions.

Acceptance criteria:

- helper reports an owned, unexpired lease;
- `pmset -g` reports `SleepDisabled 1`;
- user daemon heartbeat is active;
- battery is above minimum start threshold;
- thermal state is acceptable;
- state restoration behavior is stated in plain language.

## Epic B: native agent behavior

### B1. Codex Commute Mode

As a Codex user, I can keep the current agent useful while controlling it from ChatGPT on
my phone.

Acceptance criteria:

- rucksack attempts `codex remote-control start` when the installed CLI supports it;
- if standalone startup is unavailable but a Codex conversation is already running, the
  pack may continue after the fresh exact activation binds that conversation;
- startup failure with no running Codex conversation blocks readiness;
- first-time pairing is available through `rucksack pair codex`;
- user-level Codex hooks inject the policy only for the canonical project and provider
  session bound by the fresh `$commute-mode rucksack-…` prompt during packing;
- a `$commute-mode` skill exists for explicit activation in an existing thread;
- permission requests may be observed as passive status, but the hook returns no decision
  and the active provider permission configuration remains unchanged;
- setup tells the user to review and trust the marked entries once through Codex `/hooks`
  and records pairing and baseline phone visibility as “confirmed by you”;
- uninstall preserves unrelated `~/.codex/hooks.json` entries.

### B2. Claude Code Commute Mode

As a Claude Code user, I can continue an existing local conversation through Claude
Remote Control.

Acceptance criteria:

- the CLI recognizes that an existing interactive session needs `/remote-control`;
- the current CLI does not start a separate server-mode session and pretend it preserved
  an existing conversation;
- hooks inject context only for the canonical project and provider session bound during
  packing by the fresh `/commute-mode rucksack-…` prompt;
- `Notification` and `PermissionRequest` update rucksack’s operational state;
- a `/commute-mode` skill is installed;
- rucksack neither enables nor disables bypass-permission mode and inherits the active
  session configuration exactly.

### B3. Cursor Commute Mode

As a Cursor user, I can keep a local agent available through Cursor for iOS without
pretending the UI-first pairing step is automatable.

Acceptance criteria:

- Cursor process and current workspace are detected;
- the CLI tells the user where to enable Remote Control;
- a temporary project rule and `/commute-mode` command are created only for the active rucksack session;
- Cursor hooks provide best-effort telemetry only after the exact `/commute-mode rucksack-…`
  prompt binds the project and provider session;
- temporary Cursor files and their local `.git/info/exclude` block are removed on `unpack`,
  timeout, recovery, or preflight rollback;
- failed Cursor cleanup leaves no active policy but retains a durable project locator for
  the next `unpack` or `recover`, and a new pack cannot overwrite that locator;
- the CLI labels remote readiness “user confirmed” rather than “verified” when no stable
  API exists.
- ten unchanged pack runs reuse setup evidence without repeating pairing/trust/phone
  confirmation, while each run still requires a fresh exact-task binding;

## Epic C: safety and recovery

### C1. Battery floor

As a commuter, I want the Mac to sleep before the battery is exhausted.

Acceptance criteria:

- battery percentage and AC/battery source are sampled;
- at the configured floor, the helper restores the saved baseline;
- the release is recorded before the user daemon exits;
- a subsequent `status` explains why the session ended.

### C2. Thermal release

As a commuter, I want the Mac to stop working if the closed environment becomes unsafe.

Acceptance criteria:

- thermal pressure is sampled;
- the hardware safety monitor independently releases the sleep lease at serious/critical
  pressure or clear CPU throttling;
- CPU utilization alone does not trigger a false overheating release;
- no “override forever” option exists in the normal flow;
- an expert override, if ever added, requires an explicit time-bound value and warning.

### C3. Crash recovery

As a developer, I want normal sleep restored even if the CLI or daemon crashes.

Acceptance criteria:

- helper lease expires without heartbeats;
- helper startup restores any stale persisted lease;
- `rucksack recover` is idempotent;
- `recover` refuses to erase state until baseline restoration succeeds;
- no session file can make `SleepDisabled=1` become the new baseline.

### C4. Configuration integrity

As a developer with existing hooks and rules, I want rucksack to avoid destroying my
configuration.

Acceptance criteria:

- writes are atomic;
- a timestamped backup is created before first modification;
- rucksack entries include a stable marker;
- uninstall removes only marked entries;
- invalid JSON aborts with a clear path and no overwrite;
- file permissions are user-only where operational state is stored.

## Epic D: unpack and report

### D1. One-command cleanup

As a developer arriving home, I run `rucksack unpack`.

Acceptance criteria:

- helper baseline is restored and verified;
- daemon exits;
- temporary Cursor rule is removed;
- active-policy state is removed;
- a provider process may be stopped only after rucksack has proven ownership; the current
  Codex path records no ownership and does not stop the daemon;
- processes that existed before the session are never killed;
- final output says “Normal sleep restored”;
- manual unpack, automatic release, and recovery atomically preserve a completed-session
  report before transient session state is cleared;
- `rucksack report` retrieves the latest report without contacting the helper or network;
- human and JSON reports include duration, end kind/reason, battery and route outcome;
- mobile data is explicitly an aggregate interface estimate, partial, or unavailable, and
  is never presented as agent-only traffic or fabricated as zero.

## Failure stories

### No helper

The flow stops before any claim about lid closure and prints the one setup command.

### Hotspot is connected but captive

The flow stays at Connection and explains that Wi-Fi exists but real internet does not.

### Unplug kills the route

The flow attempts a short bounded recheck and does not say “Packed.”

### Commute route is replaced

A different live SSID, route interface, or gateway ends a strict hotspot/USB session
immediately. A temporarily missing route enters reconnect grace instead.

### Remote provider unavailable

An unreachable provider endpoint requires explicit `--allow-unverified-remote`, which is
recorded in session state. The same explicit exception may bypass missing stored phone
onboarding, but it never bypasses the fresh tokenized provider-session binding.

### Battery too low

The flow declines by default and suggests a shorter duration or actual USB-C power bank.
It does not silently weaken the battery floor.
