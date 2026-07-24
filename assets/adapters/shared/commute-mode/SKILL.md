---
name: commute-mode
description: Continue the current coding task safely while the host Mac is closed, on battery, and controlled remotely.
---

# Commute Mode

Adopt Rucksack's active Commute Mode policy. First read
`~/Library/Application Support/Rucksack/active-policy.json`. Follow its `policy` field only
when `cleanup_pending` is false and `expires_at` is still in the future. A trusted Rucksack
hook may also supply that same policy as session context. If the state is missing, invalid,
expired, or cleanup-pending, do not apply a stored policy and tell the user that Rucksack
Commute Mode is not active.

When the command includes a `rucksack-…` argument, treat it only as Rucksack's one-time
handoff confirmation. Do not repeat or persist that value.

Continue the current task under its existing instructions and active agent configuration.
Rucksack changes only the temporary focus and handoff context.
