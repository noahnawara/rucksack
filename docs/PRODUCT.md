# Product: Rucksack Commute Mode

## One-sentence product

Rucksack gives a developer one trustworthy command for handing a live local coding-agent
session from a Mac to a phone while leaving the office.

## Primary persona

**The commuter developer**

- senior individual contributor, staff engineer, founder, or hands-on engineering lead;
- works on a MacBook and already uses Codex, Claude Code, or Cursor;
- has an iPhone or another phone capable of tethering;
- starts long refactors, migrations, tests, investigations, and review loops locally;
- often needs to leave within minutes, not after a clean stopping point;
- is willing to install one transparent helper, but will not babysit power settings;
- cares more about certainty and safe recovery than about a maximal feature dashboard.

Typical commute: 20–90 minutes. Typical context: an agent has useful momentum, the user has
limited attention on the move, and the mobile connection may briefly disappear.

## Job to be done

> When I have to leave while a local coding agent is doing useful work, let me pack the
> laptop and continue steering from my phone, confident that the Mac will remain awake,
> online, safe, and recoverable.

## User anxieties

The product is not fundamentally about a `pmset` flag. It is about eliminating five
specific doubts:

1. **Did the Mac actually survive unplugging?**
2. **Is it really using my hotspot, with real internet?**
3. **Can the phone-facing agent remote still reach the host?**
4. **Will the agent get stuck asking for something I cannot safely approve?**
5. **Will the laptop overheat or run flat in the bag?**

The CLI should answer each doubt with measured evidence.

## First principles

### 1. Verify the transition, not the configuration

A checked setting is not proof. The failure happens during a state transition:

```text
external power → hotspot → battery → lid closed
```

Rucksack must guide the user through the same transition and re-check the resulting
system. “Packed” means the transition was observed and validated.

### 2. Confidence is the primary interface

The CLI should communicate three things only:

- what Rucksack knows;
- what the user needs to do now;
- what will happen automatically.

The packing story has an explicit ownership grammar:

- Rucksack work begins with `→ rucksack is`;
- user work begins with `your turn`, followed by `→` steps;
- every wait names what Rucksack is waiting for and what continues automatically with `↳`;
- measured results begin with `✓` and contain facts only;
- flavor never replaces an instruction.

The renderer enforces this grammar. Agent models never generate safety-critical CLI copy.
The words remain clear when color, symbols, animation, and terminal styling are unavailable.

Internal concepts such as IOKit, root domains, launchd, hook payloads, and network routes
belong under `--verbose`.

### 3. One obvious path

The primary command is:

```text
rucksack pack
```

The command discovers the likely agent, applies safe defaults, and asks for one physical
action at a time. Flags exist for repeatability and automation, not because setup should
feel like assembling a control panel.

### 4. Temporary capability, never permanent mutation

Closed-lid wakefulness is a lease with:

- an owner;
- a reason;
- an expiry;
- a heartbeat;
- a saved baseline;
- a recovery path.

Agent policy is also temporary. Rucksack should remove or deactivate every injected rule
when the session ends.

### 5. Safe failure means sleep

If Rucksack cannot prove that its helper, heartbeat, baseline, or safety monitors are
healthy, it should restore normal sleep. Losing a remote session is inconvenient. Leaving
a hot Mac awake indefinitely is unacceptable.

### 6. Commute Mode changes handoff, not capability

The mobile user has less attention and a less reliable connection. Commute Mode should
keep the current task moving without narrowing its workload:

- ask fewer non-blocking questions;
- state reasonable assumptions and concise checkpoints;
- run every workload required by the task, including builds, broad test suites, Docker,
  VMs, browser automation, and indexing;
- use bounded retries for unreliable connections;
- reduce workload only when the user explicitly selects `--focus low-power`.

### 7. Native integration over screen scraping

Each adapter should use the agent’s own supported extension surface:

- Codex lifecycle hooks, skills, and Remote Control commands;
- Claude Code hooks, skills, and Remote Control;
- Cursor rules, hooks, and Remote Control.

Rucksack should never inject keystrokes into a terminal or parse pixels from an IDE.

### 8. Local by default

Rucksack does not need to become another remote coding service. Provider-native remotes
carry the conversation. Rucksack manages host health and short operational status. No
repository content should transit a Rucksack relay. Version 0.1 has no Rucksack backend,
relay, or webhook transport.

## Product promise

> Rucksack will tell you it is safe to close the lid only after the Mac is on battery, the
> closed-lid lease is active, the hotspot path still has internet, and the selected agent’s
> remote handoff is ready or explicitly acknowledged.

For a strict hotspot or USB session, that promise remains bound to the verified route. A
confirmed replacement network ends Commute Mode; temporary route loss gets a bounded
reconnect grace.

## Non-goals

- emulating an AC adapter;
- replacing ChatGPT, Claude, or Cursor mobile interfaces;
- changing the provider session's permission, approval, or sandbox configuration;
- guaranteeing that any workload avoids macOS thermal pressure or throttling;
- remote desktop or SSH tunneling;
- deploying or merging code automatically;
- supporting unsupported private macOS kernel modifications;
- pretending Cursor has a CLI pairing API when it does not.

## Core metrics

### Activation

- median time from `rucksack pack` to “Packed”;
- percentage of first sessions completed without documentation;
- helper-install completion rate;
- adapter-install success and clean-uninstall rate.

### Reliability

- percentage of sessions that survive the AC→battery transition;
- remote reachability five and fifteen minutes after lid closure;
- number of false “Packed” states;
- baseline-restoration success rate;
- heartbeat-expiry restoration latency.

### Safety

- sessions released at battery floor;
- sessions released under thermal pressure;
- machines found with `SleepDisabled=1` and no valid lease;
- duration between invalid state detection and restoration.

### Agent usefulness

- sessions that complete a useful checkpoint;
- approval/input waits surfaced to the user;
- sessions whose provider permission configuration remained unchanged;
- user-rated “I could leave without thinking about it.”

## Product layers

### Layer 1: Host survival

Power lease, route verification, hotspot handoff, safety monitors, recovery.

### Layer 2: Agent handoff

Start the provider remote where a stable CLI supports it; otherwise guide the user and
record explicit phone confirmation. Always expose the exact next action.

### Layer 3: Agent behavior

Temporary native policy and telemetry adapters.

### Layer 4: Optional orchestration

Future local-to-cloud or local-to-devbox handoff. This is a separate product capability,
not a prerequisite for the first release.
