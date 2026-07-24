# ADR 0002: Sleep prevention is a lease, not a toggle

Status: accepted

## Context

A global `pmset disablesleep 1` can survive a crash and strand the Mac awake.

## Decision

Only the privileged helper may mutate `SleepDisabled`. It records the previous value,
requires heartbeats, enforces a maximum expiry, and restores on any invalid state.

## Consequences

- one-time privileged installation;
- a small root daemon must be audited;
- user processes remain unprivileged;
- crashes bias toward sleep;
- only one global lease can exist.
