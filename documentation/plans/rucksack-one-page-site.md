# rucksack one-page site

Status: implemented locally, verified, ready for a Vercel project and domain decision.

## outcome

Build one static page that lets a Mac-based developer understand rucksack in seconds,
see how packing and unpacking work, and copy one safe installation prompt into Codex,
Claude Code, or Cursor.

The page has one primary promise:

> keep your agent running on your commute

The page has one primary action:

> copy the agent prompt

## service decision

Use Vercel only.

- The site is static HTML, CSS, and a small TypeScript clipboard interaction.
- Vercel provides Git-linked previews, production hosting, and HTTPS.
- Supabase is unnecessary because there are no accounts, forms, databases, uploads,
  server functions, or persisted user data.
- No rucksack backend, relay, webhook, analytics script, or CMS is part of this page.
- GitHub Pages remains a viable alternative if preview deployments are not needed.

## target audience

The primary reader is a senior individual contributor, staff engineer, founder, or
hands-on engineering lead who:

- works on a MacBook;
- already uses Codex, Claude Code, or Cursor;
- leaves while a build, migration, review, investigation, or refactor is still running;
- wants to steer that same work from a phone during a 20–90 minute commute;
- values measured readiness and safe recovery more than configuration screens;
- will inspect source and security boundaries before installing a helper.

## essential user stories

1. As a developer leaving the office, I can understand the product before I scroll.
2. As a cautious installer, I can read the exact prompt before I copy it.
3. As a Codex, Claude Code, or Cursor user, I can see the required manual handoff.
4. As a Mac owner, I can see when normal sleep returns automatically.
5. As a security-conscious reader, I can confirm that rucksack does not change agent
   permissions or carry code through its own backend.
6. As a keyboard or screen-reader user, I can understand every step without relying on
   color or layout.

## product truth

The page may claim:

- macOS 14 or newer;
- support for Codex, Claude Code, and Cursor only;
- measured checks for the active agent, commute route, internet, and power transition;
- a bounded closed-lid safety lease and safety watcher;
- automatic unpack on defined safety and return conditions;
- no changes to provider permission, approval, bypass, or sandbox configuration;
- no rucksack backend, relay, or webhook carrying code or conversations;
- a compiler-verified alpha with no signed package currently published.

The page must not claim:

- production readiness;
- a signed install before a signed release exists;
- automatic phone pairing for every provider;
- machine verification of a user-confirmed phone step;
- broad hardware validation;
- support for agents other than Codex, Claude Code, and Cursor.

## information architecture

The page contains four content blocks and a footer.

### 1. promise and primary action

- `🎒 rucksack` wordmark
- one GitHub source link
- headline: `keep your agent running on your commute`
- one short explanation beginning with `Pack your Mac`
- one copy button
- no announcement bar, secondary navigation row, decorative status rail, or competing CTA

### 2. what packing looks like

A single chronological handoff demonstrates:

1. rucksack checks the active agent.
2. rucksack checks the commute connection.
3. rucksack checks power.
4. you unplug the Mac while the lid is open.
5. rucksack waits for battery power and continues automatically.
6. rucksack secures the closed-lid safety lease and reports `packed`.
7. you close the lid and go.

Desktop uses two lanes around one center line. Mobile uses one chronological line. The two
human steps include a visible `you` label so ownership does not depend on orange color.

### 3. supported agents

Three plain rows describe only the current integration boundary:

- Codex: starts Remote Control when the installed CLI supports it; the user reviews
  rucksack in `/hooks` and trusts it once.
- Claude Code: preserves the existing conversation through `/remote-control`; the user
  enables it inside that conversation.
- Cursor: uses Cursor for iOS Remote Control and temporary project rules; the user opens
  Agents → Remote Control and confirms phone visibility.

The section states that provider permissions, approvals, and sandbox settings are left
unchanged.

### 4. automatic unpack and install

The safety section explains that packing is bounded. Normal sleep returns on:

- `rucksack unpack`;
- the hard deadline;
- a missing watcher heartbeat;
- the battery floor;
- thermal pressure or CPU throttling;
- a confirmed route replacement.

The install section:

- repeats the compiler-verified alpha limitation;
- exposes the same copy action;
- keeps the complete prompt readable in the page;
- remains readable when JavaScript is disabled;
- gives a direct manual-copy instruction when clipboard access is denied.

### footer

- lowercase wordmark
- exact value proposition
- GitHub source and security links

## language system

- The brand is always written `rucksack`, including at the beginning of a sentence.
- The wordmark is `🎒 rucksack`, with no terminal dot.
- Repeated owner labels are not used.
- Human steps use direct verbs and a concise `you` owner only where ambiguity would
  otherwise remain.
- rucksack actions begin with `rucksack is` in the operational example.
- measured completion states the result.
- waiting names the condition and what continues automatically.
- errors name the stopped action, measured cause, and direct recovery action.
- pack/unpack language appears where it clarifies the model, not as decoration.
- flavor never replaces an instruction.
- meaning remains clear without color.
- periods remain in prose and operational output; navigation and button labels do not
  accumulate decorative punctuation.

## visual system

The design is a quiet field manual, not a generic SaaS landing page.

- Background: muted field-paper green.
- Text: deep blue-black.
- rucksack actions: dark green.
- human actions: warm orange.
- primary action: solid amber.
- operational example: one dark field with mint and orange text.
- Body and headings: Atkinson Hyperlegible Next.
- Operational text: Commit Mono.
- No gradients, glass panels, floating cards, fake dashboards, glow effects, stock
  illustrations, testimonial strips, or decorative metrics.
- No shadows or rounded-card collection.
- One shell width and one horizontal inset system align the entire page.

## responsive behavior

The CSS is mobile-first.

### 320–767 px

- 20px page inset
- single-column content
- headline capped at 52px
- full-width copy button with at least a 44px target
- one chronological packing line
- readable wordmark and one-line source navigation
- no page-level horizontal overflow
- primary copy action remains inside the first 844px viewport at 390px width

### 768–1023 px

- two-column hero introduction
- two-lane packing handoff
- two-column section layout
- headline begins at 68px and stays left aligned

### 1024 px and wider

- 64px page inset
- headline capped at 88px
- maximum content width of 82rem
- section headings and content share the same 4/8 column split
- footer and header align to the same shell

## installation prompt requirements

The canonical prompt lives in `site/src/content/install-agent-prompt.txt`. The build injects
that file into the visible page so the displayed text and clipboard payload cannot drift.

The prompt must:

- use only `https://github.com/noahnawara/rucksack`;
- inspect the current GitHub release state before changing the Mac;
- require both the signed package and its SHA-256 asset for a production install;
- preserve checksum, package-signature, Gatekeeper, helper, adapter, network, and doctor
  checks;
- stop before installation when no signed release exists;
- ask for explicit approval before a development-only source build;
- use a stable source directory and record the exact commit;
- require Rust 1.86 or newer for the source path;
- build with `cargo build --workspace --locked`;
- detect and modify only installed Codex, Claude Code, and Cursor integrations;
- never use Homebrew, npm, `cargo install`, blind `curl | sh`, `--force`, or verification
  bypass flags;
- never request administrator or hotspot passwords in chat;
- never alter provider permissions, approvals, bypass modes, or sandbox settings;
- run helper status and per-agent doctor checks;
- separate user confirmations from measured results;
- never run `rucksack pack` as an installation test;
- finish with the installed path and version or development commit, helper state,
  connection mode, adapters, warnings, and the next command.

## technical implementation

```text
site/
├── index.html
├── package.json
├── package-lock.json
├── playwright.config.ts
├── tsconfig.json
├── vercel.json
├── vite.config.ts
├── public/font-licenses/
├── src/
│   ├── content/install-agent-prompt.txt
│   ├── main.ts
│   └── styles.css
└── tests/site.spec.ts
```

- Vite builds static files.
- Strict TypeScript handles clipboard success and failure only.
- Fonts are self-hosted.
- There are no runtime package dependencies.
- Vercel headers set a restrictive content security policy.
- npm Dependabot is scoped to `/site`.
- GitHub Actions installs Chromium and runs the complete site test command.

## accessibility and resilience

- Semantic headings, sections, lists, figure, details, button, and live status region.
- Visible keyboard focus with a footer-specific high-contrast ring.
- Skip link.
- Minimum 44px mobile primary action.
- Page and full prompt remain readable without JavaScript.
- Clipboard denial opens and selects the canonical prompt.
- Forced-colors and reduced-motion support.
- No ownership meaning carried by color alone.
- No external runtime assets or scripts.

## verification

`npm test` must pass:

1. product promise, source link, primary CTA, and release honesty;
2. rendered prompt equals the canonical text file;
3. clipboard content equals the canonical text file;
4. clipboard denial gives a direct manual-copy recovery;
5. automated axe accessibility scan;
6. no page-level overflow at 320px;
7. primary mobile action size and first-viewport position at 390px;
8. readable no-JavaScript page and prompt.

Manual visual review covers:

- 1440 × 900;
- 1024 × 768;
- 390 × 844;
- 320 × 900;
- first viewport and full page;
- headline wrapping, shared edges, section rhythm, prompt disclosure, and footer.

## deployment plan

1. Create one Vercel project connected to the GitHub repository.
2. Set the project root directory to `site`.
3. Use `npm run build`.
4. Use `dist` as the output directory.
5. Keep framework detection as Vite.
6. Create a preview deployment and verify clipboard behavior over HTTPS.
7. Run Lighthouse and a keyboard pass on the preview.
8. Choose and attach the production domain.
9. Add the production URL to the GitHub repository metadata.
10. Promote only a saved, reviewed build.

No Vercel project or production deployment should be created until the owner selects the
account and domain.

## exclusions

- Supabase
- authentication
- forms or email capture
- analytics
- serverless functions
- CMS
- blog
- changelog feed
- testimonials
- GitHub star counters
- pricing
- screenshots of fake product UI
- unsupported agent claims
- automatic source-install fallback

## completion criteria

- The product and commute value are clear before scrolling.
- The first viewport has one dominant action.
- The unsigned-alpha limitation is explicit in the install section.
- `rucksack` is lowercase in all site copy and the canonical prompt.
- Human actions use direct verbs instead of a repeated handoff phrase.
- Pack/unpack language clarifies the product without becoming a gimmick.
- Mobile and desktop share one alignment system.
- The prompt is safe, readable, copyable, and release-aware.
- All browser, accessibility, type, build, dependency, and whitespace checks pass.
- Deployment remains intentionally pending until the Vercel project and domain are chosen.
