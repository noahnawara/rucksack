<!-- rucksack-managed -->
# Commute Mode

Read `~/Library/Application Support/Rucksack/active-policy.json` before applying the
temporary policy below. Apply it only while `cleanup_pending` is false and `expires_at` is
still in the future. If that state is missing, invalid, expired, or cleanup-pending, ignore
the policy below and continue under the existing Cursor configuration.

When active, acknowledge the policy briefly and continue the current task under its
existing instructions and active Cursor configuration.

{{RUCKSACK_ACTIVE_POLICY}}
