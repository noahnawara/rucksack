# Prior art and attribution

Rucksack is deliberately open about the projects that proved pieces of the design.

## Rucksack (Node implementation)

`mkrecny/rucksack` implements the original travel-oriented workflow in Node: preflight,
`pmset` baseline restoration, hotspot checks, a watchdog, battery/thermal floors, and
recovery. This Rust workspace is a redesign around a typed privileged lease, native agent
adapters, and a much narrower primary command.

The original repository is MIT-licensed.

## Amphetamine and Power Protect

Amphetamine is the mature general-purpose macOS keep-awake utility. Its Power Protect
helper documents and handles the Apple-silicon external-power transition by narrowly
allowing `pmset -a disablesleep 0|1`.

Rucksack is not intended to pretend this mechanism is novel. Its contribution is the
verified commute handoff, bounded safety lifecycle, agent behavior policy, and open
root-helper architecture.

## Lidless

`nghialuong/Lidless` demonstrates a Swift menu-bar application with a root helper, XPC,
`pmset`, and a heartbeat watchdog. It is useful confirmation that crash-safe closed-lid
control should use a privileged helper rather than repeated administrator prompts.

## Design difference

Rucksack’s differentiator is the system:

```text
agent remote
+ temporary native behavior policy
+ hotspot transition proof
+ power-source reassertion
+ lease expiry
+ battery/thermal release
+ one-command recovery
```

No code from these projects should be copied without preserving its license and
attribution. New contributions should prefer first-principles implementation against
official platform APIs.
