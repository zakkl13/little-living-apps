---
name: design-system
description: Apply and evolve a Little Living App's locked design system so the UI looks designed, not generated. Use whenever you build or change anything user-visible (a page, form, view, component, layout, styling), or when the owner asks to change the look ("make it warmer", "something like Stripe", "give it more personality", "freshen the design"). Covers four jobs — apply the locked system, enforce its anti-slop rules, offer the look once after the first screen ships, and run a guided look change on request. Skip for backend-only work (APIs, migrations, jobs).
---

# Design system — make the app look *designed*, not generated

Every instance has **one locked design system**, chosen safely at standup and recorded in
`design.lock` at the app repo root. Its **full curated Open Design package** is carried into the app
under `.lila/` (`USAGE.md`, `DESIGN.md`, `components.html`, `components.manifest.json`,
`design-tokens.json`), and upstream's machine-readable `tokens.css` is installed as the app's token
sink (`app/assets/stylesheets/tokens.css` for `rails-pwa`). Your job is to build *within* that system
— never to invent a parallel look — and to help the owner change it only when they ask.

The framework binary is `lila` (on a box: `/opt/lila/bin/lila`). Use it to browse and resolve the
catalog so you never hardcode paths or palettes.

## When this applies

Invoke on **user-visible work** (a view, page, form, component, layout, copy-on-screen) or an explicit
look-change request. **Skip** it for backend-only tasks (an API endpoint, a migration, a job) — those
have no screen, so there is nothing to design or offer.

## 1. Apply the locked system (every UI change)

1. **Read `.lila/USAGE.md` first** (the system's own guide), then `design.lock` (active `brand` +
   `source`) and `.lila/DESIGN.md` (visual intent + anti-patterns).
2. Build by **referencing the installed tokens and adapting the reference components** — never by
   introducing new colors, fonts, or spacing:
   - Reference the CSS custom properties from `tokens.css` (`var(--accent)`, `var(--fg)`, `var(--bg)`,
     `var(--border)`, …). **Never write a raw hex, a `px` font-size, or ad-hoc spacing in a view or
     stylesheet** — paste/keep the curated `:root` token block and use `var(--name)`.
   - **Adapt the reference components** (`.lila/components.html` + `.lila/components.manifest.json`)
     into ERB partials under `app/views/components/` + a token-referencing stylesheet. Reuse a
     reference recipe before inventing a new control. There is no pre-built component layer — you build
     it from the reference, consistently, so the next screen inherits it.
3. Do **not** reroll, swap, or re-pick the system on your own. The look is the app's identity from
   standup on — a git-tracked fact, changed only by job 4 below.

## 2. Enforce the anti-slop checklist (seeded by the system's own rules)

The bar is the locked brand's *own* rules, not a generic list. Read the **"Do's and Don'ts"** section
of `.lila/DESIGN.md` and reject *that brand's* named forbidden patterns/words. On top of that, hold a
small universal floor:

- Tokens, not raw hex — prove it: `! grep -REn "#[0-9a-fA-F]{3,6}" app/views app/assets/stylesheets`
  (excluding `tokens.css`) should find nothing you added.
- Real **empty / loading / error** states, never a blank screen.
- A consistent **SVG icon set** — never emoji as icons.
- The **type scale** respected; **AA contrast** on text; never the slop default of *purple gradient +
  Inter + centered card on white*.

## 3. Offer the look — once, only after there's a screen

Only when **`source == default`** in `design.lock` (a blind draw the owner hasn't weighed in on) **and**
a UI change just shipped (there is now something to look at): casually offer, *riding the delivery*,
e.g. *"btw I gave it a clean neutral look to start — want more personality? warm, editorial, bold,
something like Linear…?"* Then **set `source = invited`** in `design.lock` (whether or not they bite)
so it never fires again. If the app stays backend-only the offer never fires. This is the **only** place
the framework volunteers anything about taste — and it volunteers a question, not a look.

## 4. Guided selection / change on request

When the owner's own words ask for a different look — "make it warmer", "I want something like
Stripe", "change up our whole look":

1. **Browse the browsable pool:** `lila design list browsable` (brand · category · voice). Match the
   request against the category/voice metadata and **propose 1–3 candidates**, in the owner's terms.
2. **Confirm** which one (or reroll if they're unsure).
3. **Install the chosen system's curated assets:**
   ```bash
   eval "$(lila design draw <brand>)"          # resolves LILA_DESIGN_DIR for the chosen brand
   cp "$LILA_DESIGN_DIR/tokens.css" app/assets/stylesheets/tokens.css
   for f in DESIGN.md USAGE.md components.html components.manifest.json design-tokens.json; do
     cp "$LILA_DESIGN_DIR/$f" ".lila/$f"
   done
   ```
   Then re-fit your views to the new tokens/components (the token *names* are stable across systems,
   so most of it carries over; update any component CSS that referenced brand-specific recipes).
4. **Re-lock** with the user's choice — rewrite `design.lock` with the new `brand`, its `pool`, and
   `source = "chosen"`. (A `pinned` lock — set by `LILA_DESIGN=<brand>` — is treated like `chosen`:
   never re-pick it unless asked.)
5. Rebuild/restart, then **self-validate** (screenshot the changed pages) and report the new look.

This (plus an explicit reroll) is the **one** sanctioned path to change the look — always owner-driven,
never the framework volunteering taste. It fits the living-app model: you re-text it.

## Validate (always)

Finish every UI change by proving it adheres to the locked system: the `grep` from job 2 plus a
screenshot you actually open and compare to the system's intent. "Looks good" proves nothing — show the
concrete checks. Backend-agnostic: this works the same under Codex and Claude.
