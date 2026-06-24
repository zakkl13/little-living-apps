## Applying the design system (this app has a locked look)

This app ships with **one locked design system** (named in `design.lock` at the repo root), installed
as the **curated Open Design package** under `.lila/`. Build *within* it — never invent a parallel look.

- **Read `.lila/USAGE.md` first** — it's the system's own guide (read order, do's/avoids). Then
  `.lila/DESIGN.md` for visual intent + the "Do's and Don'ts" anti-patterns you must honor.
- **Tokens are upstream's curated `app/assets/stylesheets/tokens.css`** (`--bg`, `--surface`, `--fg`,
  `--muted`, `--border`, `--accent`, `--success`, `--warn`, `--danger`, `--font-display`, …). It is
  copied verbatim from the system — **do not hand-edit token values**. Reference everything via
  `var(--name)`; **never write a raw hex, `px` font size, or ad-hoc spacing** in a view or stylesheet.
- **Build components from the reference, not from scratch.** `.lila/components.html` (and
  `.lila/components.manifest.json` for the compact inventory) are the system's reference component
  markup + selectors. Adapt them into ERB partials under `app/views/components/` and a stylesheet that
  references the tokens — reuse a reference recipe before inventing a new control. There is no
  pre-built component layer; you create it, guided by the reference.
- **Honor the universal floor too:** real empty / loading / error states; a consistent SVG icon set
  (never emoji as icons); the type scale; AA contrast.
- Do **not** reroll, swap, or re-pick the system on your own. The look is the app's identity from
  standup on — a git-tracked fact, changed only by a user-driven selection (the design skill).
