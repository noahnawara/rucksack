# Security policy

## Reporting

Do not open a public issue for a vulnerability that could:

- leave a Mac permanently sleep-disabled;
- let an unprivileged process control the helper;
- execute arbitrary commands as root;
- overwrite unrelated Codex, Claude Code, or Cursor configuration;
- expose repository content, prompts, credentials, or hook payloads;
- bypass agent permission prompts.

Report privately to the maintainers listed in the repository security advisory page.

## Privileged boundary

The helper accepts only typed operations:

- acquire lease;
- renew lease;
- re-assert the existing lease;
- release lease;
- read status;
- recover.

It never accepts a path to execute, shell source, an environment map, or arbitrary
arguments. It invokes one fixed command shape:

```text
/usr/bin/pmset -a disablesleep 0|1
```

The socket uses peer credentials. Release helpers also validate the live client process
with Security.framework against the exact CLI identifier, Developer ID Application
certificate chain, and compiled Apple Team ID. Renew, re-assert, release, and recovery of
an active lease are restricted to its owner UID or root; lease-specific operations also
require the matching lease ID. Renewal cannot extend the persisted session deadline.
Production packages must be code signed and notarized.

Code-signature validation establishes executable identity, not user intent. A process
running as the lease owner can still invoke the legitimate signed CLI, so normal same-user
account compromise remains outside this boundary.

## Distribution integrity

Release automation publishes `rucksack-universal.pkg` and its SHA-256 checksum under stable
GitHub Release asset names. `scripts/install.sh` verifies that checksum, the package
signature, and the local Gatekeeper assessment before invoking the system installer with
administrator authentication. These paths remain release-gated until the first
production-signed package is published.

## Data handling

rucksack does not transmit code, prompts, transcripts, command output, or repository
paths to a rucksack-operated service. Version 0.1 has no rucksack backend, relay, or
webhook transport; provider-native remote products carry the coding conversation.
