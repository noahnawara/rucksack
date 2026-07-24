# Research notes

Research date: 2026-07-24.

This document records the upstream behavior the implementation depends on. Provider
features change quickly; release CI should test capabilities rather than compare only
version strings.

## macOS power management

### Idle assertions versus forced sleep

Apple Technical Q&A QA1340 distinguishes idle sleep from forced sleep. `caffeinate` and
IOPM assertions are useful for idle sleep, but lid closure is a forced condition. This is
why a normal keep-awake assertion does not solve battery-powered clamshell execution.

Primary source:

- Apple Developer, *Preventing Sleep* (QA1340):
  `https://developer.apple.com/library/archive/qa/qa1340/_index.html`

### `disablesleep`

`pmset -a disablesleep 1` changes the root-domain `SleepDisabled` state. The value is
global, privileged, and must be restored.

Primary source:

- Apple Support, `pmset` command-line power-management documentation:
  `https://support.apple.com/guide/mac-help/change-sleep-settings-mchle41a6ccd/mac`

The exact current help text should also be checked on the target machine with:

```text
man pmset
pmset -g
```

### Power-source notifications

IOKit exposes `IOPSNotificationCreateRunLoopSource`, which is the supported event source
for changes to power-source information. The helper uses this instead of pretending that
a polling loop can always win the transition race.

Primary source:

- Apple IOKit `IOPowerSources.h` API documentation.

### Amphetamine Power Protect

Amphetamine documents a specific Apple-silicon failure when external power is connected
or disconnected during Closed-Display Mode. Its Power Protect helper permits only:

```text
pmset -a disablesleep 1
pmset -a disablesleep 0
```

This is direct evidence that the correct workaround is re-applying the sleep override
across the power transition, not spoofing AC.

Source:

- `x74353/Amphetamine-Power-Protect` on GitHub.

## Codex

### Remote Control

Current developer commands include:

```text
codex remote-control
codex remote-control start
codex remote-control stop
codex remote-control pair --json
```

`start` runs the remote-control daemon in the background. Pairing JSON includes a pairing
code, manual code, environment identifier, and expiry. The feature is documented as
experimental, so the adapter must capability-test every command.

Primary sources:

- OpenAI, *Developer commands*:
  `https://learn.chatgpt.com/docs/developer-commands`
- OpenAI, *Remote connections*:
  `https://developers.openai.com/codex/remote-connections`

### Hooks and skills

Codex hooks include `SessionStart`, `UserPromptSubmit`, `PermissionRequest`, and `Stop`.
User hooks live in `~/.codex/hooks.json`. `SessionStart` and `UserPromptSubmit` can add
context. User skills live in `$HOME/.agents/skills/<name>/SKILL.md`.

Primary sources:

- OpenAI, *Hooks*: `https://learn.chatgpt.com/docs/hooks`
- OpenAI, *Skills*: `https://developers.openai.com/codex/skills`

### Rich state

Codex App Server exposes thread and turn events such as thread-status changes and
turn completion. Rucksack’s alpha uses hooks; a production adapter should use App Server
for richer, typed state where stable.

Primary source:

- OpenAI, *Codex App Server* documentation.

## Claude Code

### Remote Control

Supported entry points include:

```text
claude remote-control
claude --remote-control
/remote-control
```

The local process must remain alive. The remote path uses outbound HTTPS and generally
reconnects after brief interruptions, but an outage of roughly ten minutes may terminate
the Remote Control process.

Primary source:

- Anthropic, *Remote Control*:
  `https://code.claude.com/docs/en/remote-control`

### Hooks and skills

User hooks are stored in `~/.claude/settings.json`. Relevant events include
`SessionStart`, `UserPromptSubmit`, `Notification`, `PermissionRequest`, and `Stop`.
Skills live under `~/.claude/skills/<name>/SKILL.md`.

Primary sources:

- Anthropic, *Hooks guide*: `https://code.claude.com/docs/en/hooks-guide`
- Anthropic, *Hooks reference*: `https://code.claude.com/docs/en/hooks`
- Anthropic, *Skills*: `https://code.claude.com/docs/en/skills`

Claude’s `Stop` hook means a response/turn stopped; it does not prove that the user’s
entire job is complete.

## Cursor

### iOS and Remote Control

Cursor announced Cursor for iOS in June 2026. It can Remote Control agents on a local
computer and offers a separate handoff to cloud agents. Cursor also exposes a desktop
keep-awake setting, but host-side closed-lid/battery reliability remains the concern
Rucksack addresses.

Primary sources:

- Cursor’s official Cursor for iOS launch post and changelog, June 2026.
- Cursor’s official Remote Control product page.

### Rules

Project rules live in `.cursor/rules`. Cursor's file-backed global-home rule path is not
supported; global User Rules are configured through Cursor itself. Rucksack therefore uses
a temporary project rule plus a project command, excludes both locally through
`.git/info/exclude`, and removes them when the lease ends. `AGENTS.md` is not edited because
Commute Mode is transient host state, not repository policy.

Primary source:

- Cursor documentation, *Rules*.

### Hooks

Cursor’s hook surface includes session, prompt, shell, file-edit, response, stop, and
subagent events. The surface has changed rapidly across desktop, CLI, queued messages, and
background agents. Rucksack therefore treats Cursor hooks as telemetry, not as the host
safety boundary.

Primary sources:

- Cursor documentation and changelog entries for hooks.
- Cursor’s official community/support responses for known hook limitations.

## Validation policy

Before a release, automated compatibility checks should verify:

- command exists;
- expected help subcommand exists;
- hook config parses;
- a fixture hook receives an event;
- context output is accepted;
- remote command returns the expected structured shape;
- adapter uninstall is lossless.

A changelog entry is not enough proof that the installed provider version behaves as
expected.
