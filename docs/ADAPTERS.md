# agent support

rucksack installs exactly one file per supported agent, and nothing else. No hooks, no wrappers, no
rules, no settings changes.

| Agent | File |
| --- | --- |
| Codex | `~/.agents/skills/rucksack/SKILL.md` |
| Claude Code | `~/.claude/skills/rucksack/SKILL.md` |
| Cursor | `~/.cursor/skills-cursor/rucksack/SKILL.md` |

The file is a skill named `rucksack`. Its only job is to make "I'm leaving, pack my Mac" work as a
sentence inside a conversation: it tells the agent to run `rucksack pack`, to relay the last line,
and never to re-run it while it is waiting for a network.

## Rules it follows

- **Written only where the agent already lives.** If `~/.agents/skills`, `~/.claude/skills`, or
  `~/.cursor/skills-cursor` does not exist, that agent is not installed and rucksack skips it.
- **Marker-guarded.** Every managed file carries `<!-- rucksack-managed -->` just below its
  frontmatter. rucksack refuses to overwrite a `SKILL.md` without that marker, so a skill you wrote
  yourself is never destroyed. The marker sits *below* the frontmatter rather than above it because
  an agent reads a skill's `description` — the sentences that decide whether it is offered at all —
  only when the frontmatter opens the file. A comment above it costs the skill its trigger.
- **Best-effort.** Installation happens at the start of `pack` and can never fail it. Keeping the
  Mac awake does not depend on any agent being installed at all. It runs first so that the pack most
  in need of it — the first one, before the user's agent knows how any of this behaves — is the one
  that gets it.
- **Replaces the old name.** Releases before this one installed the same skill as `commute-mode`,
  which named an internal state rather than the product. Installing removes rucksack's own
  `commute-mode` file, and the timestamped backups an older release left beside it, so one product
  does not ship two skills and the confusing directory name does not survive on disk.

## Switching one off

`config.toml` carries a flag per agent, all on by default:

```toml
[adapters]
codex = true
claude = true
cursor = true
```

Setting one to `false` means rucksack does nothing for that agent: no skill is written, and for
Codex nothing asks its CLI anything — `pack` starts no Remote Control and `rucksack pair` says so
rather than trying. A skill an earlier pack already wrote is left in place; delete it if you want it
gone.

## An agent that is not there

None of this can end a session. rucksack ships to people who run one of these three, so a missing
agent is the ordinary case: `pack` warns, skips it, and keeps the lease. Codex specifically is only
considered found when the standalone CLI is on `PATH` or installed at
`~/.codex/packages/standalone/current/codex` — the copy inside ChatGPT.app refuses
`remote-control` outright, so treating it as a usable Codex reported success and then failed every
time. `pack --require-remote` is the one way to ask for the opposite, and it fails the pack.

Remote Control is spawned and forgotten, so what it says goes to `~/Library/Logs/Rucksack/remote-control.log`
rather than into the watcher's `daemon.log`.

## What rucksack never does

It does not read your conversations, inject instructions or policy into them, change permission,
approval, or sandbox settings, or bind a lease to a task. The lease belongs to the Mac, so agents
are not part of it — which is why an agent finishing its work cannot put the machine to sleep.

Cursor keeps its user-level skills in `~/.cursor/skills-cursor`, in the same one-directory-per-skill
layout, so rucksack installs there too. Skills Cursor does not itself manage survive in that
directory, so an unmanaged one is not pruned.
