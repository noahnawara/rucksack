# Native agent adapters

## Shared contract

Every adapter implements:

```text
detect()             Is the product installed and running?
install()            Add reversible native hooks/rules/skills.
activate(policy)     Make Commute Mode visible to the active agent.
remote_preflight()   Start, verify, or request the provider-native remote.
observe(event)       Record working/waiting/approval/completed signals.
deactivate()         Remove temporary policy.
uninstall()          Remove only Rucksack-owned configuration.
```

Adapters never:

- inject terminal keystrokes;
- scrape UI pixels;
- change the active provider session's permission, approval, or sandbox configuration;
- return an approve/deny decision from a permission lifecycle hook;
- edit repository-owned instructions without explicit opt-in;
- claim a UI-only remote is machine-verified.

## Shared policy

The same behavioral intent is rendered into each native system:

- continue the current acceptance criteria under the task's existing instructions;
- ask only truly blocking questions;
- state reasonable non-blocking assumptions and concise checkpoints;
- use bounded retries;
- run every workload the task requires, including builds, broad test suites, Docker, VMs,
  browser automation, and indexing.

The policy is parameterized by focus:

- `continue`: safely continue the current task;
- `finish`: close the current unit of work and run the checks it requires;
- `investigate`: prefer read-only analysis and produce a decision-ready report;
- `review`: inspect current changes for defects, tests, and risks;
- `low-power`: explicitly prioritize low-CPU work and defer heavy validation.

## Codex

### Native surfaces

- `codex remote-control start` starts the local daemon.
- `codex remote-control pair --json` creates a short-lived pairing code.
- `~/.codex/hooks.json` provides lifecycle hooks.
- `$HOME/.agents/skills/commute-mode/SKILL.md` provides explicit `$commute-mode`.
- Codex App Server exposes richer thread/turn state for a later adapter.

After installation, Codex requires a one-time trust review. Open `/hooks`, inspect the
Rucksack entries, and trust them. Until that review is complete, the explicit
`$commute-mode` skill still works, but automatic hook injection must not be reported as
active.

### Installed hooks

Rucksack merges marked handlers for:

- `SessionStart`;
- `UserPromptSubmit`;
- `PermissionRequest`;
- `Stop`.

The hook command is the absolute Rucksack binary:

```text
'/usr/local/bin/rucksack' hook codex
```

When no active Rucksack policy exists, the hook exits successfully with no output.

When active, `SessionStart` and `UserPromptSubmit` return:

```json
{
  "hookSpecificOutput": {
    "hookEventName": "UserPromptSubmit",
    "additionalContext": "<temporary commute policy>"
  }
}
```

`PermissionRequest` records “waiting for approval” but returns no decision, preserving the
normal Codex prompt.

### Existing session

The explicit `$commute-mode` skill is the reliable way to apply the policy immediately to
a thread that predates activation. New turns also receive the hook context.

Rucksack attempts `codex remote-control start`. If the installed CLI lacks that standalone
command or startup fails, Rucksack continues only when a Codex conversation is already
running and the user explicitly confirms that session on the phone. It fails when neither
condition is available.

### Remote ownership

The current Codex start result does not prove whether the daemon predated Rucksack.
Session state therefore records no ownership, and `unpack` never stops that daemon.

## Claude Code

### Native surfaces

- `claude remote-control` starts server mode.
- `claude --remote-control` starts an interactive remote-enabled session.
- `/remote-control` attaches an existing interactive session.
- `~/.claude/settings.json` provides hooks.
- `~/.claude/skills/commute-mode/SKILL.md` provides `/commute-mode`.

### Existing session limitation

A separate process cannot safely attach Remote Control to the exact live interactive
session. Rucksack therefore asks the user to run `/remote-control` in that session. It
does not send synthetic keys, launch a separate server, or assume another process
preserves history.

### Installed hooks

- `SessionStart` and `UserPromptSubmit`: inject the temporary policy.
- `Notification`: update waiting-for-input/permission state.
- `PermissionRequest`: update waiting-for-approval state, never decide.
- `Stop`: record a completed turn, not “the entire job is finished.”

Claude’s Remote Control process exits after an extended network outage of roughly ten
minutes. Rucksack probes the link and releases after its configured network grace period.
Version 0.1 has no Rucksack notification or webhook transport.

### Provider configuration

Rucksack does not set, clear, or override `--dangerously-skip-permissions`,
`bypassPermissions`, or any provider permission setting. It inherits the active CLI/session
configuration exactly. The `PermissionRequest` hook is passive status telemetry and returns
no approval or denial.

## Cursor

### Native surfaces

- Cursor for iOS can Remote Control agents running on the Mac.
- Cursor rules provide model instructions.
- Cursor hooks expose session, tool, shell, edit, subagent, response, and stop events.
- Cloud agents are a separate execution path and may be a future handoff target.

### Remote limitation

Current Remote Control activation is UI-first. Rucksack can detect Cursor and open or name
the required place, but it cannot honestly prove pairing through a documented CLI API.
The final handoff labels this step “Confirmed by you.”

### Temporary project rule and command

Cursor's file-backed rules are workspace-scoped; a file under `~/.cursor/rules` is not a
supported global rule. While active, Rucksack therefore writes two **untracked, reversible**
files in the current project:

```text
<project>/.cursor/rules/rucksack-commute.mdc
<project>/.cursor/commands/commute-mode.md
```

The rule uses `alwaysApply: true` for new turns. The `/commute-mode` command is the explicit
path for a conversation that was already open when Rucksack activated. In Git worktrees,
Rucksack adds a marked block to `.git/info/exclude`, so neither transient file pollutes
`git status` and no tracked ignore file is changed. All three mutations are removed on
`unpack`, lease expiry, recovery, or preflight rollback.

An unmarked file at either reserved path is never overwritten or removed.

### Hooks

Rucksack merges best-effort telemetry handlers for:

- `sessionStart`;
- `beforeShellExecution`;
- `afterFileEdit`;
- `stop`.

Cursor’s hook surface has changed quickly and has had gaps between desktop, CLI, queued
messages, and background agents. Host safety must never depend on a Cursor hook firing.

### Security

Rucksack does not use `beforeShellExecution` to build a regex firewall or add permission
rules. The active Cursor session's existing instructions and permission configuration remain
in effect unchanged.

## Configuration merge strategy

For JSON hook files:

1. parse the complete existing document;
2. reject invalid JSON;
3. create a timestamped backup on first change;
4. append entries containing a stable `rucksack hook <agent>` marker;
5. write atomically with user-only permissions;
6. on uninstall, recursively remove only marked hook objects;
7. delete empty Rucksack-created files but preserve user-created files and unrelated keys.

For Cursor project rules and commands, refuse to overwrite an unmarked file at either reserved path; modify only a marked local `.git/info/exclude` block.

## Support matrix language

Use these labels:

- **Verified**: Rucksack measured a stable machine-readable state.
- **Started**: Rucksack launched the provider command successfully.
- **Observed**: a native hook reported an event.
- **Confirmed by you**: the provider exposes only a UI state.
- **Unavailable**: a documented requirement failed.

Never collapse all five into a green “OK.”
