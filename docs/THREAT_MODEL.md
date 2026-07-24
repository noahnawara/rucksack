# Threat model

## Assets

- root execution boundary;
- global sleep configuration;
- user’s agent hook/rule configuration;
- repository path and operational state;
- remote-session availability;
- battery and thermal safety.

## Adversaries

1. A malicious local process running as the same user.
2. Another local user.
3. A compromised hook payload or project.
4. A malicious repository attempting prompt injection.
5. An attacker on the mobile network.
6. Accidental crashes, partial writes, stale state, and power loss.

## Trust boundaries

### Root helper

Trusted to run one fixed power command. It must not trust request-supplied commands,
paths, environment, or baseline values.

### User daemon

Trusted with user files and network probes, but not root. It may request release; it cannot
choose an arbitrary `pmset` argument.

### Agent hooks

Untrusted input. Hook JSON can contain repository-controlled strings and must never be
used to construct shell commands. Rucksack parses only fields needed for operational
state.

### Provider remote

Out of scope for authentication design. Rucksack relies on provider-native TLS,
authentication, pairing, and organization policy.

## Root helper controls

- Unix peer credentials via `getpeereid`;
- dynamic peer-process validation through Security.framework in release builds;
- exact `io.rucksack.cli` code identifier, Developer ID Application chain, and compiled
  Apple Team ID requirement;
- socket owned by root and an administrator group;
- one live lease;
- renew, re-assert, release, and active recovery restricted to owner UID or root;
- matching lease ID required for lease-specific mutations;
- protocol versioning;
- maximum reason length;
- bounded TTL;
- a persisted session deadline that renewal cannot extend;
- fixed absolute executable path;
- fixed arguments;
- cleared environment;
- timeout and output bounds;
- root-owned state with `0600` permissions;
- fail-safe startup recovery;
- no request-directed executable, library, or plugin loading;
- no network access.

The unsigned development helper is deliberately not a production security boundary. It
is available only from debug builds, emits a warning, and exists for local inspection.
The signed package builds the helper with a Team ID, verifies both binaries report that
Team ID after signing, and makes the release helper reject unsigned or differently signed
clients.

## Configuration controls

- parse before backup/write;
- atomic rename;
- `0600` operational files;
- stable Rucksack marker;
- backup before first mutation;
- no symlink following for reserved files;
- ownership check;
- clean removal of marked entries only.

## Hook controls

- permission hooks return no decision and do not alter provider settings;
- no shell construction from hook input;
- stdin size limit;
- output is generated from compiled policy and fixed schemas;
- transcripts and tool output are not persisted by default;
- event log stores event name, timestamp, state, and optional redacted tool category only.

## Network controls

- probes do not include repository data;
- HTTPS for provider probes;
- captive-portal probe contains no secrets;
- `--allow-unverified-ssid` still requires exact join-request evidence or interactive
  Wi-Fi-menu confirmation for a configured privacy-redacted SSID;
- strict hotspot/USB sessions bind the verified SSID, route interface, and gateway;
- a different live route identity releases immediately, while route loss receives only
  the bounded reconnect grace;
- version 0.1 has no Rucksack-operated backend, relay, webhook, or notification transport.

## Prompt-injection posture

The policy is guidance, not a security boundary. The active provider session's existing
instructions, sandbox, approval, and permission configuration remain the boundary, including
any bypass mode the user deliberately selected. A repository can attempt to override
guidance; therefore:

- Rucksack neither grants nor revokes provider permissions;
- Commute Mode adds no Rucksack-specific approval or deny rules;
- permission lifecycle hooks observe state but return no decision;
- user documentation must not claim prompt text alone guarantees safety.

## Denial of service

A local same-user process can kill the user daemon. The helper lease then expires and
restores sleep. In a release installation, an unsigned or differently signed process
cannot call the helper directly. This validates executable identity, not user intent: a
process running as the lease owner can still invoke the legitimate signed CLI.

A same-user process could invoke the hook command repeatedly; rate-limit logs and bound
input.

## Privacy

Default Rucksack state should not contain:

- prompt text;
- assistant responses;
- command output;
- file contents;
- credentials;
- pairing codes after display;
- repository remote URLs.

The completed-session report may store local operational metadata and aggregate start/end
byte counters for the verified commute interface. Rucksack does not capture packets,
destinations, hostnames, URLs, payloads, prompts, responses, command output, file contents,
or repository content for reporting.

Project directory and reports remain local and are never uploaded by Rucksack. The mobile
data value can include macOS and unrelated-app traffic, can omit traffic on other
interfaces, and is neither per-agent attribution nor carrier billing.

## Residual risks

- undocumented macOS behavior may change;
- `pmset disablesleep` is a global setting;
- thermal sampling is not a substitute for ventilation;
- a hardware fault can prevent cleanup;
- provider hook semantics may change;
- Cursor Remote Control may remain UI-only;
- a Mac closed in a bag can still become warm before the next sample.

These risks belong in release notes, not hidden in legal text.
