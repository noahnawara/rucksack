---
name: rucksack-pre-push-review
description: Review rucksack changes before they are pushed. Use when the user asks for a pre-push review, a code review, a design review, a Rust review, a diff review, or a second opinion on a branch, commit, or pull request in the rucksack workspace; when checking whether a change is small, dependable, and shippable; or when deciding whether a state transition, CLI copy, adapter change, or helper change is ready to leave the machine. Routes by path: website-only changes under site/ get a design review through rucksack-design and Impeccable, crate, asset, and script changes get the Rust and dependability review, and docs-only changes get a single check that the prose still matches the code.
---

# Rucksack Pre-Push Review

Review the change the way a maintainer who has to ship this alpha would: is the Rust good, is
the diff as small as it can be, will a commuter trust what the CLI prints, and is everything
green. Prefer one strong finding over a list of nits.

rucksack is a side project. It is not a compliance target. Do not turn this review into a
privacy audit, a threat model, or a security report. `docs/THREAT_MODEL.md` already records
that posture, and it does not need re-deriving per diff.

The one exception is dependability. The helper runs as root, flips a global macOS setting, and
the Mac may be closed in a bag. Rules that keep that recoverable stay in scope, because they are
reliability, not paperwork. The security-shaped rules that survive this posture are exactly the
bullets under `## Keep the machine dependable` and the four under `## Keep the privileged helper
boring`. Raise those when the diff touches them. Do not derive new security findings beyond them.

## Resolve the target

Default to what is about to be pushed:

```sh
git rev-parse --abbrev-ref HEAD
git log <base>..HEAD --oneline
git diff <base>...HEAD
git status --short
```

`<base>` is `origin/main` for an ordinary branch. Also accept a PR URL, a bare PR number, a
branch, or a stacked branch. For pull requests use
`gh pr view <target> --json number,title,body,baseRefName,headRefOid,url,files` and
`gh pr diff <target> --patch`. The repository is `noahnawara/rucksack`.

If `baseRefName` is not `main`, the branch is stacked. `<base>` becomes the parent branch. Review
the target against it, note the parent in one line, and do not attribute parent decisions to the
child.

If nothing is unpushed and no target was named, say so plainly and stop. Do not invent a review
of already-merged work, and do not treat untracked editor or Finder debris as a change.

Read `README.md`, `CONTRIBUTING.md`, the parts of `docs/ARCHITECTURE.md` that cover the touched
surface, and `docs/UX.md` when the diff changes CLI output. Prefer the code over the docs when
they disagree, and say which doc is now stale.

## Route by what changed

Classify the changed paths first, checking these rows in order. First match wins. Skipping the
irrelevant route is the point, not a shortcut.

```sh
git diff <base>...HEAD --name-only
```

For a PR target, classify from the `files` array of `gh pr view`, not from a local diff.

| Changed paths | Route |
|---|---|
| Any path in `crates/**`, `assets/**`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `rustfmt.toml`, `deny.toml`, `.cargo/**`, `scripts/**`, `.github/**`, `VALIDATION.md`, `SOURCE_MANIFEST.sha256` | **Project route.** If a `site/**` path also changed, run **both routes**. |
| Otherwise, any `site/**` path | **Site route.** Judge any prose that came with it under the docs question. |
| Otherwise, Markdown under `docs/**`, `documentation/**`, or the repository root | **Docs route.** One question: does the prose still match the code? Keep the finding standard, the output shape, and the verdict; this route has no command gate. |

Markdown under `assets/adapters/**` is not docs. `rucksack-core` compiles it into the binary with
`include_str!` (`crates/rucksack-core/src/policy.rs:116`,
`crates/rucksack-core/src/agent/cursor.rs:20`), so a change there is a behavior change and takes
the project route.

### Site route

For a diff confined to `site/`, do not run the Rust lenses, the crate-boundary lens, the
dependability lens, or the helper rules. They describe crates this diff cannot reach.

1. Load the `rucksack-design` skill. If the loader cannot find it by name, read
   `.agents/skills/rucksack-design/SKILL.md` directly; it is not mirrored under `.claude/skills/`.
   It is the pinned authority for rucksack's visual identity, and a site diff that drifts from it
   is the main thing worth catching.
2. Load Impeccable when it is present. Run
   `node .agents/skills/impeccable/scripts/context.mjs --target site/index.html` once, as its own
   setup requires, then follow `reference/critique.md` for the design review and
   `reference/audit.md` for accessibility, responsive, and performance checks. Impeccable lives
   outside version control, so it may be absent in a fresh checkout. When it is missing, review
   against `rucksack-design` alone and say that Impeccable was unavailable.
3. Review in the browser at the viewports and motion sampling points `rucksack-design` specifies.
   That skill is the authority if this one drifts. Do not approve a visual change from reading
   the diff.
4. Verify with the site gate:

```sh
cd site && npm ci && npm audit --audit-level=high && npm run build && npx playwright install --with-deps chromium && npm run test:e2e
```

Three things under `site/` are not design surface, and each is worth one check:

- `site/vercel.json` carries the Content-Security-Policy and the other response headers.
- `site/src/content/install-agent-prompt.txt` is the install instruction a visitor hands to an
  agent. Read a change to it against `INSTALL.md` and `SECURITY.md`, not as copy.
- `site/package.json` and `site/package-lock.json` are dependency decisions, gated by
  `npm audit --audit-level=high` rather than by `deny.toml`.

Ranked goals 1, 2, and 4 below apply, along with the `## Fewer lines wins` ladder minus its
crate-specific examples. Goals 3 and 5 do not.

### Project route

Run `## What this review optimizes for`, `## Fewer lines wins`, `## Judge the Rust`,
`## Judge the experience`, `## Keep the machine dependable`, `## Keep the privileged helper
boring`, `## Crate boundaries`, `## Required completeness`, and `## Green before push`. Skip the
design lenses; a Rust diff does not need a typography opinion.

`## Finding standard`, `## Question gate`, `## Output shape`, `## Posting to GitHub`, and
`## Last check` apply to every route.

## What this review optimizes for

Ranked. When two goals collide, the higher one wins:

1. **It works and stays green.** The change does what it claims, and the verification gate passes.
2. **Fewer lines.** The best version of most diffs is a smaller one.
3. **Good Rust.** Types carry the meaning, errors are honest, no speculative machinery.
4. **Good experience.** The user knows what rucksack knows, what to do now, and what happens automatically.
5. **Dependable transitions.** Any failure ends with normal sleep restored and state recoverable.

Everything else is optional commentary. Say it in one clause or leave it out.

Two additions are never gold-plating: the recovery copy for a new user-facing state, and a test
for a new state transition, parser, or rollback. Goals 1, 4, and 5 outrank goal 2 there. Ask for
the smallest version that still carries them.

## Fewer lines wins

Run this ladder before proposing anything bigger:

1. Can the goal be met by deleting code, or by narrowing scope?
2. Does `std`, macOS, `clap`, `serde`, `chrono`, or an already-present crate own this concern?
3. Does an existing helper in `rucksack-core` cover it without a new module?
4. Is the new trait, enum variant, config knob, or flag backed by a second real caller today?
5. If the change is a deliberate shortcut, is its ceiling obvious enough that nobody later reads it as an oversight?

Treat these as cost, not neutrality: a new workspace dependency, a trait with one implementor, a
wrapper that only forwards, hand-rolled parsing or retry or date logic, a config option nobody
sets, and a flag with one call site.

A new dependency must clear `deny.toml` as written: allowlisted license, no wildcard version,
crates.io only. Adding to the allowlist is a decision, not a detail. Call it out.

## Judge the Rust

These lenses describe what this codebase actually does. Do not ask a diff to adopt a pattern the
workspace does not use.

- **Finite sets are enums.** `SessionPhase` (`crates/rucksack-core/src/state.rs:17`), `AgentKind`,
  and `EvidenceInvalidationReason` (`crates/rucksack-core/src/onboarding.rs:87`) already are, so
  do not re-litigate them. The deliberate exception is the provider hook event name, which stays a
  `String` matched by lowercased literal with an unknown-event fall-through
  (`crates/rucksack-cli/src/hooks.rs:43`, `378-402`). A new event there needs a test, not an enum.
  Everywhere else, if a value can be matched on, make the compiler match it.
- **Identifiers.** Session, lease, and request ids are bare `Uuid` throughout
  (`crates/rucksack-core/src/state.rs:41-42`, `crates/rucksack-core/src/protocol.rs:12-33`), and
  identity is enforced by explicit comparison rather than by the type system. Do not ask for
  newtypes. Do check that every id comparison a new path adds is against the right field, because
  `id` and `lease_id` are swappable without a compile error.
- **Parse untrusted JSON once, at the edge.** Hook stdin and provider output arrive shapeless.
  Convert to a typed struct immediately. Do not let `serde_json::Value` or
  `HashMap<String, Value>` travel into the protocol, session state, or durable files. A generic
  serialization helper may hold `Value` internally; each caller keeps its own type.
- **Errors say what failed.** `anyhow` with context is the workspace default, inside
  `rucksack-core` as much as at the CLI edge, so a new `anyhow::bail!` in core is not a finding.
  Reach for a `thiserror` type only when a caller must branch on the failure, as
  `SessionStateWriteConflict` does (`crates/rucksack-core/src/state.rs:165`, downcast at
  `crates/rucksack-cli/src/daemon.rs:463`). A swallowed error, a bare `.ok()`, or a `_ =` on a
  fallible write is a finding when it hides a state the user needs to see.
- **No `unwrap`, `expect`, or slicing panic on a live path.** The daemon heartbeat, the hook
  entrypoint, and every helper operation must not panic on hostile or merely surprising input. A
  panic in a constructor at startup is fine. A panic in a loop is not.
- **`unsafe` and `libc`.** All 14 blocks in the workspace are a single FFI call with a checked
  return, spread across all three crates (`crates/rucksack-core/src/files.rs:117`,
  `crates/rucksack-core/src/system.rs:328`, `crates/rucksack-cli/src/flow.rs:1909`,
  `crates/rucksack-helper/src/server.rs:235`). Hold a new block to that shape: one FFI call, its
  return checked, no raw pointer retained past the call, no `unsafe` widened over surrounding
  logic. Ask for a `SAFETY:` comment only where the block dereferences a pointer.
- **Deadlines.** The helper's lease TTL, hard deadline, and 5-second watchdog are wall-clock
  `chrono` comparisons with no monotonic backstop (`crates/rucksack-helper/src/lease.rs:102-103`,
  `327-332`), bounded by the 24-hour ceiling; the daemon's network-outage grace is the one timer on
  `Instant` (`crates/rucksack-cli/src/daemon.rs:538-553`). Do not ask for a monotonic rewrite. Do
  check that a new timeout either rides `Instant` like the outage grace or is clamped the way
  `clamped_expiration` is, so a clock change cannot push a deadline further out.
- **Byte counters and casts.** Interface counters reset and wrap. An `as` cast or a naive
  subtraction that can produce a wrong number should produce an unavailable result instead.

## Judge the experience

The CLI is the product. `docs/UX.md` is the copy spec, in `## Information hierarchy`, the
primary-flow transcript, and `## Copy rules`; `docs/PRODUCT.md` restates the same grammar in one
list. Hold the diff to both:

- rucksack's own work begins with `→ rucksack is`. The user's work begins with `your turn`,
  followed by its own `→` steps.
- Every wait names what it is waiting for and what continues automatically, with `↳`.
- Measured results begin with `✓` and contain facts only.
- Flavor never replaces an instruction.

Then:

- **Never print a state that was not measured.** This is the product. A guessed route, a guessed
  battery reading, or a cheerful "connected" that was inferred rather than probed is a blocker,
  because the whole promise is that `packed` was observed.
- **Partial beats fabricated.** Missing baseline, changed interface, or reset counter yields
  unavailable or partial, not a plausible number.
- **Every new user-facing state needs four copies:** success, waiting, warning, and recovery. A
  state with no recovery copy strands the user on a train.
- **Errors are actionable.** Say what happened, and what to do. An override flag mentioned in an
  error must exist and must actually help.

This lens covers the CLI. Anything under `site/` belongs to the site route.

## Keep the machine dependable

Raise these only when the diff touches them. Each is a blocker when broken:

- **Every global mutation records a baseline first and has an idempotent rollback.** Persist
  before you mutate, verify after, and keep the persisted record until restoration is confirmed.
- **Failure biases toward normal sleep.** An orderly release transitions through `Releasing`; a
  lost helper heartbeat goes to `Failed` and lets the helper's lease TTL restore sleep
  (`crates/rucksack-cli/src/daemon.rs:140`, `crates/rucksack-helper/src/lease.rs:98-111`). Either
  terminal is fine. Returning to `Active` after an inconsistency, or a failure path that leaves no
  terminal phase at all, is not.
- **The hard session deadline is not renewable.** A renewal path that can push it out is a defect.
- **One owner per invariant.** Sleep state belongs to the helper. Session state belongs to the
  user daemon. A new file write needs the existing advisory lock, revision check, and atomic
  temp-plus-rename, not a fresh bespoke write.
- **`pack`, daemon release, `unpack`, and `recover` share one terminal-operation lock.** A new
  entry point that can start or finalize a session must take it.
- **Adapters are reversible.** Back up before the first mutation, mark what rucksack owns,
  preserve unrelated user entries, and remove only marked entries. Prove uninstall restores the
  rest.
- **Never touch the provider's permission, approval, or sandbox configuration.** Permission hooks
  observe and return no decision.
- **Never build a shell command from hook input.**

## Keep the privileged helper boring

Four rules. A diff that breaks one is a blocker; there is nothing to discuss:

1. No arbitrary-command interface, ever. Fixed absolute path, fixed arguments, cleared environment.
2. No HTTP client, no config merging, no shell, no request-directed filesystem access in the helper.
3. Owner UID required for lease mutation, matching lease ID for lease-specific operations.
4. Startup restores a persisted baseline before opening the socket.

Beyond those four, do not open a security audit of the helper. Its controls are enumerated in
`docs/THREAT_MODEL.md` under `## Root helper controls` and its design in `docs/ARCHITECTURE.md`.
The Rust lens and the dependability rules still apply to helper code this diff touches.

## Crate boundaries

| Concern | Owner |
|---|---|
| Types, configuration, session state, parsers, policy rendering, agent detection, adapter documents, adapter install/remove, the remote-onboarding registry, subprocess and network probes, paths, atomic files, protocol | `rucksack-core` |
| Interactive flow, daemon, hooks, privileged helper install and launchd, helper client | `rucksack-cli` |
| Socket, lease, fixed `pmset`, baseline, watchdog, power events | `rucksack-helper` |
| Marketing surface | `site/` |

`rucksack-core` is privilege-free but not side-effect-free. It owns every subprocess
(`crates/rucksack-core/src/system.rs:61`), the `reqwest` probe
(`crates/rucksack-core/src/network.rs:302`), and all mutation of the user's agent config
(`crates/rucksack-core/src/agent/json_hooks.rs:119`). Judge a new core side effect on whether it
is bounded, cleared-environment, and reversible, not on whether it is a side effect at all.
Privilege-free work landing in `rucksack-helper` is worth one finding.

## Required completeness

Wanting a diff to cover more ground is not a finding. Wanting it to be *correct* is. Block only
when an untouched surface must change for this change's own goal to hold, such as a renamed field
still read by a persisted file or hook, a second entry point that bypasses the gate this diff
added, or a behavior change that contradicts `README.md`, `docs/ARCHITECTURE.md`, `docs/UX.md`, or
`docs/PRODUCT.md` without updating it.

Test necessity, not adjacency: is the other surface required for *this* change to work, or is it
just nearby?

## Green before push

CI runs on every pull request, with no path filters, so every job below is a required check on a
Rust diff. For `site/**`, use the site gate in the site route. A docs-only diff has no gate; say
so instead of running Cargo.

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUCKSACK_TEAM_ID=CI00000000 cargo clippy --workspace --all-targets --release --locked -- -D warnings
RUCKSACK_TEAM_ID=CI00000000 cargo test --workspace --release --locked
RUCKSACK_TEAM_ID=CI00000000 cargo build --workspace --release --locked
```

The release build is the step that links the helper reading
`option_env!("RUCKSACK_TEAM_ID")`, so a packaging or signing diff that skips it can be called
green and land red.

The four debug commands run on both `macos-14` and `ubuntu-24.04`. When the diff adds or moves
`cfg(target_os = "macos")` code, a clean local macOS run is not proof of green: say the Linux leg
is unverified, or ask for it.

When the diff touches `Cargo.toml`, `Cargo.lock`, or `deny.toml`, run the gate CI runs:

```sh
cargo deny --locked check
```

It needs `cargo install cargo-deny` and is not installed by default. Say so if it was unavailable.

A new behavior with no test is a finding when the behavior is a state transition, a parser, or a
rollback. A new test for a getter is noise.

Power, launchd, and closed-lid behavior cannot be verified by reading the diff. Ask what was
actually run, on which macOS and hardware. Never suggest a closed-lid test inside an insulated
bag; require a ventilated desk and a short lease.

`VALIDATION.md` records the source inventory and the checks that passed. `SOURCE_MANIFEST.sha256`
covers only the crate, docs, assets, scripts, and root-config tree; it excludes `site/**`,
`INSTALL.md`, `documentation/**`, and `.agents/**`. Both take the project route. Regenerate them
when a covered file is added, removed, or changed.

## Finding standard

Each finding states the decision, the concrete failure it causes, the evidence by file and line,
and a smaller or cleaner alternative. Drop anything that cannot name a failure.

- **Blocker.** Breaks the fail-safe direction, corrupts durable state, prints an unmeasured
  state, strands a user with no recovery, breaks one of the four helper rules, or lands red.
- **Major.** Works today, but the types, ownership, or state machine will mislead the next change.
- **Strategic.** Fine to land. Worth an ADR in `docs/adr/` because it constrains later work.

Verify against the current diff. A stale review comment or a previous bot finding is evidence, not
a verdict. Do not report merge conflicts, rebase needs, or branch staleness as findings.

## Question gate

Ask at most three questions, each tied to a concrete line in the diff, and only when the answer
would change the review. Ask when the tradeoff is "ship it smaller" versus "handle this state
now", when the intended shape depends on where the alpha is going, or when a transition's intent
is genuinely unreadable from the code.

Write the questions in the `**Architecture Questions**` section and still choose a verdict in the
same response. Do not stop and wait for answers. Give the verdict the current diff earns and put
the dependency inside the question. If nothing is material, omit the section.

## Output shape

Start at `**Findings**`. No title header, no leading blank line.

```markdown
**Findings**

- **[Major] Title** - `path/to/file.rs:120`. Decision, the failure it causes, the smaller alternative.

**Architecture Questions**

- One question tied to a specific line.

**Better Design Path**

- Short proposal, including work outside this diff when that is the honest answer.

**What Looks Sound**

- Only choices worth preserving on purpose.

**Residual Risk**

- What could not be verified: power, launchd, closed-lid, hardware, or the Linux CI leg, and what was run instead.

**Review Verdict**

- Approve, Request changes, or Comment only, with one sentence of rationale.
```

When the branch is stacked, open with `**Push Scope**` instead:

```markdown
**Push Scope**

- Stacked on `<parent-branch>`; reviewed against it, not `main`.

**Findings**
```

When both routes apply, replace the single `**Findings**` heading with `**Findings - Site**` and
`**Findings - Project**`, then give one `**Review Verdict**` for the push.

Choose one verdict. Never write "Approve if" or "Request changes if". Omit sections that add
nothing, except `**Residual Risk**`, which is required whenever the diff touches power, launchd,
or closed-lid behavior. When there is nothing to raise, say so and keep the residual risks.

## Posting to GitHub

Only when asked. Post a review, not an issue comment:

1. `gh pr view <target> --json number,headRefOid,url,baseRefName,title,author`
2. Anchor each actionable finding to one changed line with `path`, `line`, `side: "RIGHT"`, `body`.
3. Put scope, questions, design path, and the verdict in the review body. Do not repeat inline text.
4. Map the verdict to `event`. GitHub rejects `APPROVE` and `REQUEST_CHANGES` on a self-authored
   PR with HTTP 422, and nearly every non-dependabot PR in this repo is self-authored, so post
   `COMMENT` and state the verdict in the body. Use `APPROVE` or `REQUEST_CHANGES` only on a PR
   someone else opened.
5. Write the body to a file and build the JSON from it. Check with `jq -r '.body' review.json` that
   newlines are real newlines, not literal `\n`.
6. `gh api repos/noahnawara/rucksack/pulls/<number>/reviews --method POST --input review.json`

Report the result plainly: "Posted the review on PR 18 with COMMENT."

## Last check

The two failure modes of this review are asking for a bigger diff and reporting what CI already
enforces. `cargo fmt` and `clippy -D warnings` are required checks, so a formatting or lint note
is wasted review. Reread the findings for both before posting.

When a request conflicts with this skill, keep the change shippable and dependable first, then say
what was traded away instead of quietly widening the diff.
