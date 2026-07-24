# rucksack one-page site

Status: implemented and deployed at
[rucksack.wtf](https://rucksack.wtf).

## product language

- primary promise: `switch to your hotspot. keep your agent running.`
- support: `pack your Mac. keep working from your phone.`
- differentiator: `don’t move the project. move yourself.`
- route: `office wifi → phone hotspot`
- outcome: `seamless commute`
- primary action: `copy setup prompt`
- verified state: `packed`

[docs/UX.md](../../docs/UX.md) is the canonical language contract. the public site,
README, repository metadata, and CLI summary reuse this vocabulary.

## audience and job

the primary reader already runs Codex, Claude Code, or Cursor locally on a Mac and needs
to leave while useful work is still running. they want the shortest path from office wifi
to a phone hotspot without cloning the project, rebuilding an environment, or moving
secrets into another machine.

the page must answer three questions in order:

1. what does rucksack do?
2. why not move the project to the cloud?
3. what does `packed` prove?

## page structure

### 1. hero and commute pass

the first viewport contains:

- the lowercase rucksack wordmark and pixel backpack mark;
- one GitHub link with the repository star count;
- the primary promise and support line;
- a compact commute pass showing `office wifi → phone hotspot`;
- the `seamless commute` outcome;
- one `setup prompt` action.

the pass is evidence, not a dashboard. it stays smaller than the headline on desktop and
uses extra internal whitespace on mobile.

### 2. cloud contrast

`don’t move the project. move yourself.` explains the differentiator without a feature
grid:

```text
cloud      clone → configure → add secrets
rucksack   pack → connect hotspot → go
```

### 3. packed proof

`packed means you can leave.` names the four mandatory states:

- power — running on battery
- route — phone hotspot has internet
- agent — current task observed
- phone — access confirmed by you
- sleep — closed-lid lease active and bounded

the section also states the safety boundary: permissions stay unchanged and rucksack
never relays repository code.

### footer

the footer repeats only the wordmark, GitHub link, and security link.

## product truth

the page may claim:

- macOS 14 or newer;
- support for Codex, Claude Code, and Cursor;
- a guided move from office wifi to a phone hotspot;
- measured power, route, internet, and sleep-lease checks;
- an exact live task activation that is observed by rucksack;
- phone visibility that is explicitly confirmed by the user;
- bounded closed-lid protection and automatic restoration of normal sleep;
- unchanged provider permission, approval, and sandbox configuration;
- no rucksack backend, relay, or webhook carrying repository code;
- a compiler-verified alpha with no signed package currently published.

the page must not claim automatic phone pairing, universal remote reachability,
production readiness, or a signed install before a signed release exists.

## visual system

the design is a compact commuter pass on a quiet dark field.

- DM Sans carries product copy.
- Doto carries route labels and measured state.
- acid green marks the active route and primary action.
- a low-contrast green glow follows the page from hero to proof.
- three continuous backgrounds create sections without rules or decorative lines.
- spacing and alignment carry hierarchy; the pass is the only bordered object.
- the pixel backpack is a route marker, not an emoji or illustration.
- motion explains issue, route, and packed state; it never blocks the setup action.
- reduced-motion users receive the same final state without choreography.

the layout is mobile-first. a shared page shell aligns header, hero, both supporting
sections, and footer. on wide screens the content remains centered inside that shell
instead of drifting toward viewport edges.

## service and implementation

Vercel is the only hosted service required. Supabase is unnecessary because there are no
accounts, forms, databases, uploads, or persisted application data.

- Vite builds static HTML, CSS, and TypeScript.
- the site uses no frontend framework because the only interaction is clipboard copying.
- Vercel provides Git previews, production hosting, HTTPS, Analytics, and Speed Insights.
- fonts are self-hosted.
- the content security policy allows only the production assets and telemetry endpoints
  the site uses.

the canonical setup prompt lives in
[`site/src/content/install-agent-prompt.txt`](../../site/src/content/install-agent-prompt.txt).
the build injects that file into the readable page and clipboard payload so they cannot
drift.
