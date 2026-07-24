---
name: commute-mode
description: Continue the current coding task safely while the host Mac is closed, on battery, and controlled remotely.
---

# Commute Mode

Adopt Rucksack's active Commute Mode policy. First read
`~/Library/Application Support/Rucksack/active-policy.json`. If it exists, follow the `policy`
field exactly. A trusted Rucksack hook may also supply that same policy as session context. If
neither source contains an active policy, tell the user to run `rucksack pack`.

Continue the current task under its existing instructions and active agent configuration.
Rucksack changes only the temporary focus and handoff context.
