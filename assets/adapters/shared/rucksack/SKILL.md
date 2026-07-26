---
name: rucksack
description: Keep this Mac awake and online while it is closed and on battery, so running tasks survive the commute. Use for "rucksack", "rucksack pack", "pack my Mac", "I'm leaving", "heading out", "have to go", "commute mode", "switch to my hotspot", "keep working while the lid is closed", "I'm back", and "unpack".
---
<!-- rucksack-managed -->

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

- **Just run it.** Do not ask which agent, which network, how long, or whether to proceed. It needs
  no arguments.
- **Relay, don't narrate.** The last line is the verdict. Pass it on in one sentence — "Packed,
  you're good to go" — and stop. Do not restate every line or explain what a lease is.
- **If it waits, do not sit on it.** When `pack` says it is waiting for a network, run it in the
  background with its output going to a file, tell the user the one thing it asked for, and add that
  the lid has to stay open until it says `Packed`. Then check the file every half minute until the
  last line appears. A second `pack` is never the fix.
- **`unpack` prints a trip line first.** Read it back with the numbers exactly as printed. Never
  round one, and never supply one that is not there.
- **Warnings are not failures.** "Remote Control did not start" means the Mac is still packed and
  the work still runs. Mention it once, in passing.

Only bother the user when the command actually fails, and then give them the one line it printed.

## Switching the network yourself

Do this only if you can screenshot and click this Mac's screen. If you cannot, skip the whole
section — `pack` handles the network by itself, and says what it needs.

No command-line tool can join an Apple Instant Hotspot or read a Wi-Fi password out of the keychain,
so clicking is the only way this gets done for the user instead of by them.

Work fast and accept less certainty than usual: **two screenshots per switch, one retry, then hand
it over.** Do not re-verify what you can already see.

**Try the Wi-Fi menu bar icon first.** One click opens a list that labels itself — `Personal
Hotspot`, then `Known Network`, then `Other Networks` behind a submenu. That structure is the whole
decision: the row you want is always in one of the first two groups, never in the submenu.

If the click is refused, or the list will not open, fall back to System Settings › Wi-Fi, which takes
more steps but works the same way. If that is refused too, tell the user which network to pick and
carry on — that is not a failure, it is just slower.

**Leaving** — before `rucksack pack`, never while it is running.

1. Open the list. Note which row has the checkmark: that is what the user comes back to.
2. Click the row under `Personal Hotspot`. If there is no such row, just run `rucksack pack`.
3. Run `rucksack pack`. Do not pass `--here` — pack proves the hotspot itself, and `--here` would
   throw that proof away in favour of your screenshot.
4. If `pack` refuses — flat battery, hot Mac — put the row from step 1 back before you report.

**Coming back** — only if `unpack` says the Mac is still on the phone.

1. Open the list and click the row under `Known Network`. This Mac already has credentials for it,
   so it simply connects.
2. If there are several, pick the one the user has been on today, and say which you picked.
3. If there are none, or you cannot tell, name what you can see and ask which one.

Never join a network under `Other Networks`. Never type a Wi-Fi password. Never accept a sign-in
page. Those are the user's to do.

## The first trip

`unpack` prints one extra line the first time a trip completes, and only ever that once. When you
see it, ask the user — with your own question UI, the one that gives them buttons instead of a line
of chat to skim past. In Claude Code that is the `AskUserQuestion` tool; use the equivalent in
whatever you are. If you have none, one short line of chat is fine.

Ask whether it worked for them, and whether they would star the project. If yes, run `rucksack star`
— it stars through the `gh` CLI they are already signed in to, and opens the page for them to click
if it cannot. If no, drop it permanently and never raise it again.

Ask once, after their work is running again. Never before, never twice.

## Options, if they ask for them

`--for 90m` shortens the session. `--hotspot "Name"` names the network. `--usb` uses iPhone USB
tethering. `--here` keeps the network the Mac is already on — for a travel router or an Android
hotspot, where rucksack has no way to recognise the switch by itself. That is the whole surface.

## What it does not do

The lease covers the whole Mac, not one conversation, so every running task keeps going and a task
finishing does not end it. rucksack does not change your instructions, tools, or permissions — carry
on with the current task exactly as before.
