# product: rucksack

## one-sentence product

rucksack moves this Mac onto a phone hotspot and holds it awake on battery with the lid
closed, so work that is already running survives the walk to the train.

## Primary persona

**The commuter developer**

- senior individual contributor, staff engineer, founder, or hands-on engineering lead;
- works on a MacBook and already uses Codex or Claude Code;
- has an iPhone or another phone capable of tethering;
- starts long refactors, migrations, tests, investigations, and review loops locally;
- often needs to leave within minutes, not after a clean stopping point;
- is willing to install one transparent helper, but will not babysit power settings;
- cares more about certainty and safe recovery than about a maximal feature dashboard.

Typical commute: 20–90 minutes. Typical context: the work has useful momentum, the user
has limited attention on the move, and the mobile connection may briefly disappear.

## Job to be done

> When I have to leave while something useful is still running, let me pack the laptop
> and walk out, confident that the Mac stays awake on my hotspot and hands normal sleep
> back safely on its own.

## User anxieties

The product is not fundamentally about a `pmset` flag. It is about eliminating five
specific doubts:

1. **Is it really on my hotspot, with real internet?**
2. **Will it stay awake once the lid is shut and the cable comes out?**
3. **Can my phone still reach the work?**
4. **Will the laptop overheat or run flat in the bag?**
5. **Can I get normal sleep back afterwards?**

`pack` answers these by measuring rather than asserting, with two exceptions: `--here`
takes the user at their word that this network is the commute network, and Remote
Control is started but only checked under `--require-remote`.

## First principles

### 1. Verify the transition, not the configuration

A checked setting is not proof. The failure happens during a transition:

```text
current network → hotspot → lid closed
```

“The internet works” is not evidence, because the office network the user is walking
away from also works, and accepting it packs a Mac that goes offline at the front door.
Arrival is proven by the Wi-Fi name matching the saved hotspot, by the default route
visibly leaving the interface or gateway it started on, by a gateway that only an iOS
Personal Hotspot serves — `172.20.10.1`, or `192.0.0.1` when the carrier is IPv6-only —
by macOS reporting that it joined the network rucksack asked for, or by `--here` — and in
every case the route must then actually reach the internet.

There is no unplug step. Packing while plugged in is allowed, and the helper re-asserts
the override when it observes the power source change.

### 2. Confidence is the primary interface

The CLI should communicate three things only:

- what rucksack knows;
- what the user needs to do now;
- what will happen automatically.

It has four verbs: `step` for what just happened, `done` for the verdict on the last
line, `warn` for something worth knowing that did not stop anything, and `detail`, which
only `--verbose` prints. No narration, no chapters, no house voice.

Agent models never generate safety-critical CLI copy. The words remain clear when color,
symbols, animation, and terminal styling are unavailable.

Internal concepts such as IOKit notifications, `SleepDisabled`, launchd, and network
routes belong under `--verbose`.

### 3. One obvious path

The primary command is:

```text
rucksack pack
```

It takes no arguments and asks nothing: no agent to pick, no network to name, no
confirmation. When it does need the user it prints exactly one instruction — opening
Wi-Fi settings when that instruction is about Wi-Fi — and then waits for as long as it
takes, ticking every 30 seconds. It never aborts and never asks for a re-run. The only
question rucksack ever asks comes at the end of the first successful `unpack`, when the
user is back at a desk. Flags exist for repeatability and automation, not because
packing should feel like assembling a control panel.

### 4. Temporary capability, never permanent mutation

Closed-lid wakefulness is a lease with:

- an owner uid;
- a reason;
- a hard expiry, plus a shorter helper TTL;
- a heartbeat that renews it;
- a saved baseline;
- a recovery path.

The helper refuses to acquire unless sleep is already normal, so its rollback target is
never ambiguous. Nothing is left behind: deleting the session file is the resting state.

### 5. Safe failure means sleep

If rucksack cannot prove that its helper, heartbeat, baseline, or safety monitors are
healthy, it restores normal sleep. A `pack` that fails partway rolls back what it did,
the watcher releases the lease when the host stops being safe, and the helper's own TTL
restores sleep if heartbeats stop arriving. Losing a remote session is inconvenient.
Leaving a hot Mac awake indefinitely is unacceptable.

### 6. Integrate through the files an agent already reads

Agent integration is one marker-guarded skill file, written to
`~/.agents/skills/rucksack/` and `~/.claude/skills/rucksack/`, and only where that
agent's directory already exists. No hooks, no rules files, no keystrokes injected into
a terminal, no pixels parsed out of an IDE. A file rucksack does not own is never
overwritten, and installing retires the older `commute-mode` skill it wrote itself.

### 7. Local by default

rucksack does not need to become another remote coding service. Provider-native remotes
carry the conversation. rucksack manages host health and short operational status. No
repository content should transit a rucksack relay: there is no rucksack backend, relay,
or webhook transport, and the only request a packed session makes is a captive-portal
probe to `captive.apple.com`.

## Product promise

> rucksack says `Packed.` only after nothing else already owns this Mac's sleep setting,
> the battery is not at the sleep floor, the Mac is not thermally throttled, the power
> helper is installed and holding a bounded lease, the route is on a network proven to
> be the commute network and reaching the internet, and the safety watcher has sent its
> first heartbeat.

Remote Control is started, not gated: if it does not start, `pack` warns and the Mac is
still packed, unless `--require-remote` was passed.

After that, the lease survives everything that does not matter. It belongs to the Mac,
not to a conversation, so losing the network does not end it and an agent finishing its
task does not end it. Six things end it: `rucksack unpack`, the time limit, the battery
floor, serious or critical thermal pressure or actual throttling, the helper heartbeat
failing, and three consecutive failures to read the battery while on battery.

## Non-goals

- emulating an AC adapter;
- replacing the provider's own mobile interface;
- changing an agent's instructions, tools, or permissions;
- rejoining a hotspot that drops after the lid closes — the lease survives it, but
  nothing reconnects the network for you;
- guaranteeing that any workload avoids macOS thermal pressure or throttling;
- remote desktop or SSH tunneling;
- deploying or merging code automatically;
- supporting unsupported private macOS kernel modifications.

## Core metrics

### Activation

- median time from `rucksack pack` to “Packed”;
- percentage of first sessions completed without documentation;
- helper-install completion rate;
- helper clean-uninstall rate.

### Reliability

- percentage of sessions that survive the unplug into the bag;
- remote reachability five and fifteen minutes after lid closure;
- number of false “Packed” states;
- baseline-restoration success rate;
- helper-TTL restoration latency.

### Safety

- sessions released at battery floor;
- sessions released under thermal pressure;
- machines found with `SleepDisabled=1` and no valid lease;
- duration between invalid state detection and restoration.

### Usefulness

- sessions that complete a useful checkpoint;
- user-rated “I could leave without thinking about it.”

## Product layers

### Layer 1: Host survival

Power lease, arrival proof, safety watcher, recovery. The load-bearing layer: a failure
here always stops the pack, and a half-finished pack rolls back.

### Layer 2: Agent handoff

Start Codex Remote Control as a fire-and-forget child, and print a pairing code on
request with `rucksack pair`. A failure here warns rather than fails, unless
`--require-remote`.

### Layer 3: Vocabulary

One skill file, so “pack my Mac” works as a sentence inside a conversation. No policy,
no telemetry, no injected rules.
