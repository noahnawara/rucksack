---
name: rucksack
description: Keep this Mac awake and online while it is closed and on battery, so running tasks survive the commute. Use for "rucksack", "rucksack pack", "pack my Mac", "I'm leaving", "heading out", "commute mode", "switch to my hotspot", "keep working while the lid is closed", and "unpack".
---

# rucksack

The user is walking out of the door and wants their running work to survive it. Run the command,
relay the result in one short line, and let them go.

| They say | You run |
| --- | --- |
| "I'm leaving", "pack my Mac", "rucksack pack" | `rucksack pack` |
| "is it still going?" | `rucksack status` |
| "I'm back" | `rucksack unpack` |

## How to behave

Be easy-going. This is a two-second command, not a project.

- **Just run it.** Do not ask which agent, which network, how long, or whether to proceed.
  It needs no arguments.
- **Relay, don't narrate.** `pack` prints about four lines and the last one is the verdict. Pass
  that verdict on in one sentence — "Packed, you're good to go" — and stop. Do not restate every
  line, explain what a lease is, or add caveats.
- **Do not re-run it.** If `pack` is waiting for the user to choose a network, it says so and then
  keeps waiting; it finishes on its own. A second `pack` is never the fix.
- **One instruction at a time.** If it does need the user, it will have printed exactly one thing
  to do. Say that one thing, nothing else.
- **Warnings are not failures.** "Remote Control did not start" means the Mac is still packed and
  the work still runs. Mention it once, in passing.

Only bother the user when the command actually fails, and then give them the one line it printed.

## Options, if they ask for them

`--for 90m` shortens the session. `--hotspot "Name"` names the network. `--usb` uses iPhone USB
tethering. `--here` keeps the network the Mac is already on — reach for it if the user says they
are already tethered and `pack` is still waiting for a switch. That is the whole surface.

## What it does not do

The lease covers the whole Mac, not one conversation, so every running task keeps going and a task
finishing does not end it. rucksack does not change your instructions, tools, or permissions —
carry on with the current task exactly as before.
