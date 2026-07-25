# agent support

rucksack installs exactly one file per supported agent, and nothing else. No hooks, no wrappers, no
rules, no settings changes.

| Agent | File |
| --- | --- |
| Codex | `~/.agents/skills/rucksack/SKILL.md` |
| Claude Code | `~/.claude/skills/rucksack/SKILL.md` |

The file is a skill named `rucksack`. Its only job is to make "I'm leaving, pack my Mac" work as a
sentence inside a conversation: it tells the agent to run `rucksack pack`, to relay the last line,
and never to re-run it while it is waiting for a network.

## Rules it follows

- **Written only where the agent already lives.** If `~/.agents` or `~/.claude` does not exist,
  that agent is not installed and rucksack skips it.
- **Marker-guarded.** Every managed file starts with `<!-- rucksack-managed -->`. rucksack refuses
  to overwrite a `SKILL.md` without that marker, so a skill you wrote yourself is never destroyed.
- **Best-effort.** Installation happens after a successful `pack` and can never fail it. Keeping
  the Mac awake does not depend on any agent being installed at all.
- **Replaces the old name.** Releases before this one installed the same skill as `commute-mode`,
  which named an internal state rather than the product. Installing removes rucksack's own
  `commute-mode` file so one product does not ship two skills.

## What rucksack never does

It does not read your conversations, inject instructions or policy into them, change permission,
approval, or sandbox settings, or bind a lease to a task. The lease belongs to the Mac, so agents
are not part of it — which is why an agent finishing its work cannot put the machine to sleep.

Cursor has no skill file: rucksack installs nothing for it. `rucksack pack` still keeps a Mac
running Cursor awake, because the lease is host-wide.
