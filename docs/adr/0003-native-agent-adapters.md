# ADR 0003: Use native agent extension points

Status: accepted

## Context

rucksack needs to alter agent behavior and observe waiting/completion state.

## Decision

Use provider-native skills, hooks, rules, and remote-control commands. Never synthesize
keystrokes or scrape UI state. When an API is UI-only, ask for explicit user confirmation
and label it accordingly.

## Consequences

- separate capability/version checks per provider;
- configuration merge/uninstall code is critical;
- Cursor remains less automatable than Codex or Claude Code;
- native semantics may evolve and require compatibility tests.
