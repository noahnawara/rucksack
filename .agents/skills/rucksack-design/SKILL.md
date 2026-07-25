---
name: rucksack-design
description: Apply and extend Rucksack's confidence-first visual system. Use when Codex designs, implements, reviews, or polishes any Rucksack website, landing page, product UI, documentation surface, component, responsive state, interaction, or motion; when a request should match the current rucksack.wtf identity; or when checking work for design-system drift.
---

# Rucksack Design

Design Rucksack as calm operational confidence: a quiet near-black field, one acid signal, editorial space, human language in DM Sans, machine evidence in Doto, and motion that verifies state without disturbing layout.

Use Impeccable for the design and critique workflow when it is available. Treat this skill as the pinned Rucksack-specific authority.

## Start from truth

Before changing a Rucksack surface:

1. Read `docs/PRODUCT.md` for the promise, persona, anxieties, and safety boundaries.
2. Inspect `site/src/styles.css` and `site/index.html` for the incumbent visual truth.
3. Inspect the target at desktop and mobile sizes. Preserve the existing identity unless the user explicitly asks to replace it.
4. State the user-visible goal and the invariants that must survive.

Prefer the implementation over stale documentation. Update this skill when the shipped system intentionally changes.

## Hold the point of view

Build around these principles:

- **Confidence is the interface.** Show what Rucksack knows, what the user does next, and what happens automatically.
- **One obvious path.** Give each surface one dominant promise and one dominant action.
- **Evidence beats decoration.** Use labels, routes, checks, and the `packed` result to make state legible.
- **Calm beats spectacle.** Let scale, contrast, alignment, and whitespace create drama.
- **Local and temporary feel trustworthy.** Avoid cloud-dashboard tropes, surveillance imagery, or claims that imply Rucksack relays code.
- **Safe failure remains visible.** Never hide errors, recovery, no-JavaScript access, or reduced-motion behavior.

## Use the visual language

### Color

Use the semantic palette already defined in `site/src/styles.css`:

| Role | Token | Value | Use |
|---|---|---:|---|
| Field | `--night` | `#080a07` | Page and section background |
| Raised surface | `--pass` | `#151a12` | Commute pass and code surfaces |
| Primary text | `--ink` | `#f4f5eb` | Headlines, decisive labels, verified facts |
| Secondary text | `--muted` | `#a9b09f` | Explanation, support, metadata |
| Signal | `--acid` | `#ceff39` | Primary action, active route, verified state |
| Signal hover | `--acid-soft` | `#e0ff82` | Hovered acid controls only |
| Signal ink | `--acid-ink` | `#0a0d08` | Text and icons on acid |
| Structural line | `--line` | `#3c4535` | Component borders |
| Chapter line | `--chapter-line` | `rgb(60 69 53 / 58%)` | Quiet section boundaries |
| Error | `--error` | `#ff9f80` | Actionable failures |

Reserve acid for proof and action. A full-width primary control may use it; ordinary prose and decorative backgrounds may not. Do not add competing accent colors, gradients, glows, or translucent glass.

Treat `#d5d8cc` and `#c0c5b8` as local neutral steps on the current landing page, not global tokens. Preserve them when maintaining that page; promote a new semantic token only after the same role repeats.

Keep page sections on the same night field. Separate chapters with large vertical space and a quiet one-pixel line, not alternating background slabs.

### Typography

- Use **DM Sans Variable** for promises, explanations, actions, and human-readable facts.
- Use **Doto Variable** for machine evidence: route names, state labels, section labels, metadata, and small navigation.
- Keep display headings bold, tightly tracked (`-0.035em`), and open (`1.2` line-height).
- Keep body copy around `0.95–1.05rem` with `1.42–1.45` line-height.
- Keep machine labels around `0.72–0.76rem`, bold, with `0.035–0.045em` tracking.
- Keep line lengths short. Balance headlines and pretty-wrap supporting prose.
- Use fixed physical widths for layout-critical display copy whose wrap must stay prescribed across font outcomes. Keep the primary face on `font-display: optional` so a late font never moves visible text.
- Preserve lowercase brand voice. Retain proper casing for names such as Mac and GitHub.

Do not turn all copy into terminal text. The contrast between humane DM Sans and evidentiary Doto is the system.

### Space and layout

- Use a centered shell capped at `70rem` with `clamp(1.1rem, 4vw, 3.5rem)` gutters.
- Use one column below `60rem`.
- At `60rem` and above, align recurring sections to one grid: `minmax(0, 1fr) 23rem` with a `4–7rem` responsive gap.
- Keep the promise wider and visually heavier than the pass or proof column.
- Give persuasive marketing chapters generous vertical room: roughly `9–10.5rem` on compact layouts and `13–14.5rem` on wide layouts. Use denser, task-appropriate rhythm for operating and reading surfaces.
- Keep related gaps compact inside components; spend the large space between ideas.
- Keep primary mobile actions at least `44px` tall. At the `390px` reference viewport, keep the pass narrower than the headline. At `320px`, both may use the available column width, but the pass must remain visually subordinate and fully contained.

Do not fill empty space with decoration. Whitespace is the primary section material.

### Geometry and iconography

- Use one-pixel borders and clipped ticket corners for the commute pass.
- Use square, pixel-sharp marks with `shape-rendering: crispEdges`.
- Use the backpack mark as a functional route/brand symbol, not a repeated pattern.
- Keep surfaces flat: no drop shadows, generic rounded cards, pill containers, or nested card stacks.
- Prefer straight alignment, compact labels, and deliberate asymmetry over dashboard chrome.

### Components and patterns

Preserve these system patterns:

- **Wordmark:** acid clipped mark plus bold lowercase name.
- **Commute pass:** machine route and state above a human outcome, ending in one full-width action.
- **Evidence list:** terse Doto key paired with a strong DM Sans fact; reserve acid for the final result.
- **Contrast path:** show the costly default and the simpler Rucksack route without adding feature-card chrome.
- **Chapter boundary:** shared field, large whitespace, one subtle horizontal rule.
- **Primary action:** acid bar, dark ink, square inset icon, clear copied/error states.

Extract a shared component or token only after the same intent appears at least three times. Do not generalize a one-off composition.

## Make motion prove something

Use motion as state verification, never as spatial theatre:

- Reveal the pass over `620ms` with `cubic-bezier(0.16, 1, 0.3, 1)`.
- Move from `opacity: 0.62`, `blur(6px)`, and `saturate(0.7)` to a crisp final state.
- Keep the pass geometry fixed throughout the reveal. Do not translate, scale, clip, or spring the card.
- Run the two-pixel acid scan line down and back up over `1450ms`, delayed `520ms`, with `cubic-bezier(0.65, 0, 0.35, 1)` to communicate verification.
- Use `150ms` transitions for hover color, `120ms` for pressed feedback, and `260ms` for a small icon response.
- Run entrance and verification motion once. Do not loop ambient motion.
- Remove every nonessential animation and transition under `prefers-reduced-motion: reduce`.

Test at the first frame, a middle frame, and the settled frame. Compare element bounds across frames; a visual effect must not cause layout movement.

## Write with operational clarity

- Lead with the outcome: `packed means you can leave.`
- Prefer short concrete clauses: `pack → connect hotspot → go`.
- Use contrast sparingly to clarify a decision: `don’t move the project. move yourself.`
- Name measured facts, not abstractions: `phone hotspot has internet`.
- Keep flavor subordinate to instruction.
- Do not invent product capabilities, safety guarantees, or privacy claims. Check `docs/PRODUCT.md`.
- On persuasive entry surfaces, keep one promise, one distilled proof object, and one action in the first viewport.

## Preserve resilience and access

Require all of the following:

- Use semantic landmarks, headings, lists, buttons, figures, and descriptions before ARIA patches.
- Keep a visible skip link and a `3px` high-contrast focus treatment.
- Keep controls keyboard-operable and touch targets at least `44px`.
- Announce action results with a live status region. Make errors visible and actionable.
- When a surface offers prompt copying, keep the complete prompt readable and copyable when clipboard access or JavaScript fails.
- Never rely on acid color alone to communicate state.
- Keep page-level horizontal overflow at zero at `320`, `800`, `959`, and `960px`.
- When the commute pass is present, keep its route and state phrases on one line at `320px`.
- Show the correct final state with zero running animations under reduced motion.
- Preserve strict CSP; do not add inline scripts for visual bootstrapping.

## Review in the browser

For every meaningful visual change:

1. Run the local site and capture screenshots at `390 × 844` and `1440 × 1000`.
2. Inspect the first viewport, section rhythm, alignment, wrapping, pass hierarchy, and final state.
3. Capture important motion at start, midpoint, and rest.
4. Verify no overflow, no font-driven text jump, no layout-moving animation, and no contrast or focus regression.
5. Run the existing typecheck, build, Playwright, accessibility, no-JavaScript, and reduced-motion checks.

Do not approve a change from code inspection alone.

## Reject drift

Reject or revise:

- multiple accent colors or acid used as decoration;
- alternating section backgrounds, gradients, glass, glow, or shadow;
- generic SaaS cards, pill-heavy UI, dashboards, or feature grids;
- extra calls to action competing with the primary action;
- body copy rendered as terminal text;
- spatial hero entrances, bounce, parallax, or looping ambient motion;
- late font swaps, font-relative constraints that change a prescribed display wrap, or visible load reflow;
- tiny mobile actions, wrapped pass labels, or horizontal scrolling;
- motion without a reduced-motion final state;
- clever copy that obscures ownership, evidence, recovery, or safety.

When a new requirement conflicts with this system, preserve product truth and accessibility first, then explain the visual tradeoff instead of silently diluting the identity.
