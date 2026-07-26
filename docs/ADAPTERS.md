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

## What rucksack never does

It does not read your conversations, inject instructions or policy into them, change permission,
approval, or sandbox settings, or bind a lease to a task. The lease belongs to the Mac, so agents
are not part of it — which is why an agent finishing its work cannot put the machine to sleep.

Cursor keeps its user-level skills in `~/.cursor/skills-cursor`, in the same one-directory-per-skill
layout, so rucksack installs there too. Skills Cursor does not itself manage survive in that
directory, so an unmanaged one is not pruned.
