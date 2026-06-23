## Applying the design system (this app has a locked look)

This app ships with **one locked design system** (see `design.lock` at the repo root) already
rendered into a real baseline. Build *within* it — never invent a parallel look.

- **Design tokens are CSS custom properties in `app/assets/stylesheets/tokens.css`** (`--color-*`,
  `--font-*`, `--space-*`, `--radius-*`, `--shadow-*`). They are generated from the locked system and
  include a dark-mode block. **Do not hand-edit individual token values** and **never write a raw hex,
  `px` font size, or ad-hoc spacing in a view or stylesheet — reference a token.** Changing the look is
  a deliberate, user-driven action handled by the design skill, which re-renders this file.
- **Build views from the component layer**, not from scratch: the partials in `app/views/components/`
  (button, input/form, card, nav, empty-state, list-row) and `app/assets/stylesheets/components.css`.
  Reuse and compose them; if you need a new component, add it to that layer in the same token-driven
  style so the next screen inherits it.
- **Respect the locked system's own rules.** The active system's full spec is carried into the app at
  `.lila/DESIGN.md` (the brand is named in `design.lock`). Its "Do's and Don'ts" section lists *this*
  brand's named anti-patterns — honor them, plus the universal floor: real empty / loading / error
  states; a consistent SVG icon set (never emoji as icons); the type scale; AA contrast.
- Do **not** reroll, swap, or re-pick the system on your own. The look is the app's identity from
  standup on — a git-tracked fact, not a per-task decision.
