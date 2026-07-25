# Threat model

## Assets

- root execution boundary;
- global sleep configuration;
- the agent skill file rucksack writes;
- session and configuration state on this Mac;
- remote-session availability;
- battery and thermal safety.

## Adversaries

1. A malicious local process running as the same user.
2. Another local user.
3. Any administrator-group process, which can reach the helper socket.
4. Text in an agent’s context attempting to use rucksack to change what that agent may do.
5. An attacker on the network the Mac is probing.
6. Accidental crashes, partial writes, stale state, and power loss.

## Trust boundaries

### Root helper

Trusted to run one fixed power command. It must not trust request-supplied commands,
paths, environment, or baseline values.

### Safety watcher

Trusted with user files and network probes, but not root. It may request release; it cannot
choose an arbitrary `pmset` argument.

### Agent skill

Outbound only. rucksack writes one marker-guarded document and reads nothing back: there are
no hooks, no decisions returned to any provider, and no permission changes.

### Provider remote

Out of scope for authentication design. rucksack relies on provider-native TLS,
authentication, pairing, and organization policy.

## Root helper controls

- Unix peer credentials via `getpeereid`;
- socket owned by root and the administrator group, mode `0660`;
- one live lease;
- renew, re-assert, release, and recovery restricted to the owner UID or root;
- matching lease ID required for lease-specific mutations;
- protocol versioning;
- maximum reason length of 256 bytes;
- TTL bounded to 30–300 seconds;
- a persisted session deadline, capped at 24 hours, that renewal cannot extend;
- fixed absolute executable path;
- fixed arguments;
- cleared environment;
- a five-second timeout and a 64 KiB output bound on the power command;
- request size, concurrent connection, and per-connection I/O limits;
- root-owned state with `0600` permissions;
- fail-safe restoration on startup, and on persisted state that does not validate;
- no request-directed executable, library, or plugin loading;
- no network access.

A helper compiled with `RUCKSACK_TEAM_ID` — which is how the notarized package is built —
also validates the calling process through Security.framework against the exact
`io.rucksack.cli` identifier, the Developer ID Application certificate chain, and that Team
ID. A helper built from source has no Team ID and authenticates by peer UID alone, and says
so at startup. That is deliberate: it is what makes `cargo build --release` usable, and it
means any administrator-group process on such an installation can drive the helper. What that
grants is the authority to switch one `pmset` setting — the same authority as running
`sudo pmset -a disablesleep` — bounded by the lease TTL and the non-renewable deadline.

Code-signature validation establishes executable identity, not user intent: a process running
as the lease owner can still invoke the legitimate signed CLI.

## State controls

- atomic replace, then `fsync` of the file and its directory;
- `0600` operational files inside `0700` directories;
- refusal to modify a symlinked state path;
- a version field on config and session state, rejected rather than guessed at when unknown;
- an advisory lock serialising `pack`, `unpack`, and watcher updates;
- configuration keys from older releases load and are ignored rather than failing a command;
- session state rucksack cannot parse never blocks `unpack`, which deletes it;
- a stable `rucksack-managed` marker on the skill file: rucksack refuses to overwrite a file
  it does not own, and removes only files carrying that marker.

## Network controls

- the probe carries no user, session, or repository data: it is a plain GET with a
  `rucksack/0.1` user agent;
- arrival on the commute network is proven by the Wi-Fi name matching the saved hotspot, by
  the default route leaving its baseline interface or gateway, by the `172.20.10.1` gateway
  that only an iOS Personal Hotspot serves, by a join macOS confirmed, or by `--here`, which
  is the user saying this network is the commute network — and in every case the route must
  then actually reach the internet;
- "the internet works" is never proof on its own, because the office network the user is
  walking away from also works;
- the probe is plain HTTP to `captive.apple.com` and requires Apple’s exact success page and
  final host, so whoever controls the network can make it pass or fail. That decides only
  whether `pack` proceeds and what `status` reports; a probe result never releases, extends,
  or acquires a lease;
- the lease is host-scoped, so no network change ever releases it. Losing the network is
  recorded in the session and nothing more: a train entering a tunnel must not put the Mac to
  sleep;
- rucksack operates no backend, relay, webhook, or notification transport.

## Prompt-injection posture

The skill is guidance, not a security boundary. The agent’s existing instructions, sandbox,
approval, and permission configuration remain the boundary, including any bypass mode the
user deliberately selected. Therefore:

- rucksack neither grants nor revokes agent permissions;
- rucksack installs no hooks and returns no decision to any provider;
- the skill says which command to run and to relay one line of its output, and states that
  the lease covers the whole Mac; it changes nothing about how an agent may act;
- user documentation must not claim prompt text alone guarantees safety.

## Denial of service

A local same-user process can kill the watcher. The lease then expires within its TTL and the
helper restores the sleep setting it recorded before acquiring. The same baseline is restored
when the helper cannot re-arm the override after a power-source change.

In a signed package installation, a process that is not the signed CLI cannot call the helper
at all. In a source build any administrator-group process can, and the worst it achieves is
holding sleep off until the TTL or the 24-hour deadline elapses. Flooding the socket is
bounded to refused connections rather than unbounded work, and refusing service to the CLI
does not keep the Mac awake: an unrenewed lease expires on its own.

## Privacy

rucksack’s own state records operational facts only: the saved network name, the current route
interface, battery percentage, timestamps, the owning UID, the watcher PID, and the last event
or release reason. It does not contain:

- prompt text;
- assistant responses;
- file contents;
- credentials;
- pairing codes, which are printed and never written.

One exception is worth naming: Codex Remote Control is started as a child process with
rucksack’s daemon log as its standard output and error, so whatever that command writes lands
in `~/Library/Logs/Rucksack/daemon.log`.

Session state and logs stay on this Mac and are never uploaded by rucksack.

## Residual risks

- undocumented macOS behaviour may change;
- `pmset disablesleep` is global: rucksack refuses to start when something else already owns
  it, and reasserts the override every five seconds while a lease is held, so rucksack and a
  second closed-lid utility would contend for one switch;
- thermal sampling is not a substitute for ventilation, and a Mac closed in a bag can become
  warm between samples;
- a hardware fault can prevent cleanup;
- the internet probe is plaintext HTTP and can be spoofed by whoever controls the network;
- Codex Remote Control is not supervised after it starts, so it can stop without rucksack
  noticing.

These risks belong in release notes, not hidden in legal text.
