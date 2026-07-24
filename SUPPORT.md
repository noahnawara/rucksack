# sustaining rucksack

the rucksack CLI and helper work without an account or paid service and send no rucksack
telemetry. the public website uses Vercel Analytics and Speed Insights. donations should fund
work that benefits every user: macOS compatibility testing, signing and notarization, hardware
test coverage, provider-adapter maintenance, documentation, and security review.

## Product rules for donations

- Core safety, recovery, and all agent adapters stay open source.
- No feature is withheld to manufacture a subscription.
- No CLI or helper telemetry is added merely to optimize fundraising.
- The CLI never interrupts `rucksack pack` with a donation prompt.
- A support message may appear only after a successful `unpack`, at most once per installed major
  version, and must be permanently dismissible.
- Sponsor links are configured by the maintainers in release metadata rather than hard-coded into
  privileged code.

## Suggested channels

Use GitHub Sponsors or Open Collective for recurring support and a transparent project ledger for
hardware, signing, and test-device expenses. Release artifacts must remain downloadable without a
sponsor account.
