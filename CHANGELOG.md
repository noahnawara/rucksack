# changelog

all notable changes to rucksack will be documented here.

## [Unreleased]

### Added

- Rust-native CLI, core library, and privileged macOS helper.
- A host-scoped closed-lid lease: it belongs to the Mac, so every running task benefits and
  a task finishing never ends it.
- `pack` with no setup command — the first run installs the helper and remembers the hotspot.
- A resumable hotspot handoff: when macOS cannot join, rucksack opens Wi-Fi settings, says
  what to pick, and waits indefinitely rather than aborting or asking for a re-run.
- Arrival proven by network name, a default-route change, an iOS hotspot gateway, a confirmed
  join, or an explicit `--here` — and only once the route reaches the internet.
- Battery, thermal, hard-deadline, and heartbeat safeguards, with recovery folded into
  `unpack` so restoring normal sleep never dead-ends.
- Thermal pressure read from public `ProcessInfo.thermalState` in the unprivileged watcher.
  `pmset -g therm` reports only Intel-era counters, which Apple silicon never populates, so
  it is kept as a second source rather than the only one.
- A `rucksack` skill for Codex, Claude Code, and Cursor, so "pack my Mac" works in a conversation.
- Universal packaging, signing, and notarization, with a checksum-verifying installer script.
- Canonical `pack`/`unpack` commands; former lifecycle names are rejected.
- rucksack changes no agent instructions, tools, or permissions.
- A wind-down warning ten minutes before the end. Every running task stops the moment the Mac
  sleeps, mid-step, and until now the only warning was the silence afterwards. `status` leads with
  it and the skill says what to do: stop starting new work and write the state to disk. It is not a
  release condition, and plugging in calls it off — a Mac on mains power is not ending soon, and
  saying otherwise would be a state nobody measured.

### Changed

- `pack` reports whichever limit ends the session first, instead of the configured ceiling and the
  battery floor. A duration chosen months ago and a threshold answer neither "how long have I got?"
  nor anything else, and on a commute the battery is nearly always the shorter of the two, so a Mac
  with an afternoon of charge was told it had a day. There is no measured drain at second zero, so
  the opening figure is the scaled macOS estimate `status` already borrows.
- The setup prompt on the website and in the README is 19 lines instead of 29, and 193 words
  instead of 311. It opened by explaining that it was a prompt and where to paste it, which both
  surfaces already say around it, and it spent four lines on a star request the installed skill
  already owns and asks better.
- The setup prompt no longer runs anything but the install when it is pasted into a terminal
  instead of an agent. Command names were wrapped in backticks, and a backtick pair is command
  substitution: pasted into zsh, the old text executed `rucksack star` and `rucksack unpack` out
  of the middle of two sentences. The line warning against pasting it into a terminal never
  prevented that — removing the line changes nothing, and removing the backticks fixes it.
- The internet probe asks `curl` instead of carrying `reqwest`, taking the lockfile from 152
  packages to 94 and a clean release build from 11.4s to 7.2s. One plain GET does not need tokio,
  hyper, and tower. The environment is cleared, so `http_proxy` and `~/.curlrc` cannot rewrite a
  request whose whole purpose is to describe this Mac's own path to the internet, and
  `--proto '=http'` makes the plain-HTTP promise a property of the call rather than of a feature
  flag left off.
- `SOURCE_MANIFEST.sha256` covers every tracked file. Its path list used to be read back out of
  itself, so it could only ever describe what was already in it: the website, `INSTALL.md`, and
  four of the eight files the release script rewrites were invisible to it.
- Only the settings you actually changed are written to `config.toml`. Writing every value out on
  the first pack turned each default into a decision, so a default improved in a later release
  could never reach anyone who had used rucksack once.
- `helper status` and `pack` say when the installed helper is not the one this rucksack would
  install, comparing the binaries rather than their version strings. Two builds of the same tag
  report the same version and can still differ, which is what happens when `helper install` runs
  before a `cargo install --force` replaces the binary underneath it.
- The website's install prompt asks the agent to copy the departure instructions into the file it
  loads every session. Pasted into one conversation they stopped applying when it ended.
- The battery floor is 10%, down from 15%. The old floor ended a commute with a sixth of the
  battery unused, and macOS does not begin its own low-power warnings until 10% either.
- `status` has a battery figure from the first heartbeat. Measuring a rate takes three readings,
  so the opening minutes of a session had only the lease clock to report — the one number certain
  to be wrong, at the moment someone is deciding whether to walk away. macOS has an estimate by
  then and `pmset` is already read every heartbeat, so that figure is borrowed until rucksack has
  measured one of its own, scaled first from time-to-empty to time-to-floor.
- `status` reports whichever limit ends the session first, rather than the lease clock alone. On
  a commute the battery is nearly always the smaller of the two, and a day of lease on an
  afternoon of charge was reported as a day. The battery figure is projected from drain actually
  observed, marked with `~`, and withheld until two drops have been measured.
- `unpack` lets go of the phone. A Mac that arrives somewhere new while still tethered stays on
  the hotspot, because macOS will not abandon a connection that still works; cycling the radio
  makes it choose again from what is in range. Personal Hotspot is a fallback in that choice, so
  a known network nearby wins, and a Mac that is nowhere it knows lands back on the phone.
- The internet probe no longer negotiates TLS to fetch a plain-HTTP page, and every wait now has
  a clock on it.

### Fixed

- A one-trip `--for` no longer becomes a permanent default. `pack --for 45m` wrote the 45 into the
  same configuration that remembering your hotspot saves a few lines later, so the first short pack
  on a new install silently made 45 minutes the length of every pack after it.
- A session that ran out its own clock gets its trip line back. The helper's watchdog checks the
  hard deadline every five seconds and the watcher wakes every thirty, so on an ordinary expiry the
  helper got there first and the watcher read "no active lease" as a failure. The record then kept
  claiming a lease nothing held: the next `pack` refused against a deadline already in the past, and
  `unpack` printed nothing at all.
- `pack` no longer refuses over a session whose watcher is dead, telling the user to `unpack`
  something nothing was holding at exactly the moment packing was what the Mac needed.
- A Mac finishing its charge is no longer read as time-to-empty. Three substring tests were trying
  to spell one word and still missed the top-off state a plugged-in Mac sits in around 99%.
- `status` no longer reports a dead session as packed.
- The `[adapters]` flags in `config.toml` are read again. `codex = false` had been discarded at
  load, so nothing downstream could obey it.
- Codex is considered installed only when the standalone CLI is on `PATH` or under
  `~/.codex/packages/standalone/current`. The copy inside ChatGPT.app refuses `remote-control`,
  so treating it as usable reported Codex present on almost every Mac and then failed every call.
- An agent this Mac does not have can no longer end a pack. It warns and is skipped; only
  `pack --require-remote` still treats a missing Codex as fatal.
