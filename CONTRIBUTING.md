# Contributing

rucksack changes a global macOS power-management setting and may run while a laptop is
closed in a bag. Reliability and safe rollback matter more than feature count.

## Ground rules

1. Never add an arbitrary-command interface to the privileged helper.
2. Every global mutation must record a baseline and have an idempotent rollback.
3. Failure must bias toward restoring normal sleep.
4. Agent adapters must preserve existing user configuration and support clean removal.
5. Never alter the active provider session's permission, approval, or sandbox configuration;
   permission hooks return no decision.
6. Do not claim a network, remote, or power state that was not actually measured.
7. New user-facing states need copy for success, waiting, warning, and recovery.

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Power and launchd integration tests must run on a disposable or dedicated Mac. Never run
closed-lid tests inside an insulated bag. Use a ventilated desk setup and a short lease.

## Pull requests

Describe:

- the user problem;
- the state transition being changed;
- rollback behavior;
- failure injection performed;
- macOS version and hardware tested;
- agent versions tested, where relevant.

For adapter changes, include before/after copies of the relevant config and proof that
uninstall restores unrelated entries.
