# ADR 0001: Do not spoof AC power

Status: accepted

## Context

Closed-display mode can collapse when an Apple-silicon Mac changes from external power to
battery. A tempting framing is “fake a charger.”

## Decision

Rucksack will not spoof the system power source. It will control only the sleep decision
through a time-bounded `SleepDisabled` lease and will require the transition to battery
while the lid is open.

## Consequences

- no private SMC/kernel dependency;
- simpler threat model;
- honest battery/thermal behavior;
- actual external power still requires USB-C PD hardware;
- power transition reassertion remains necessary.
