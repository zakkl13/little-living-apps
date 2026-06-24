## Runtime conventions (this app is a Rails 8 app)
- The app is a **Rails 8** project (SQLite + the Solid stack, Hotwire/Turbo, structured as a PWA).
  Build with the grain of Rails 8 defaults — reach for built-ins before adding gems.
- **Reload mode:** edits to existing code go live on the **next request** — no restart. Structural
  changes (a new gem, an initializer, a route, a migration) DO need a restart:
  `sudo systemctl restart "${LILA_APP_SERVICE:-lila-app@primary}"`. Run migrations with
  `bin/rails db:migrate`.
- **Auth:** use Rails' built-in authentication (`bin/rails generate authentication`).
- **Reserved path:** `/_agent/*` is reserved. Never route app paths under it.
- If the app isn't scaffolded yet, create it with `lila-new-app` (a minimal Rails 8 + PWA app).

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
  standup on — a git-tracked fact, changed only by a user-driven selection.

When your change is user-visible, your self-validation must also show it stayed *within* the system —
with concrete evidence, never a bare "looks good":

1. **Tokens, not raw values.** Prove your CSS/ERB references the tokens (`var(--accent)`, …), not
   hardcoded colors/spacing: `! grep -REn "#[0-9a-fA-F]{3,6}" app/views app/assets/stylesheets`
   (excluding `tokens.css`) should find nothing new you added (report the grep result).
2. **No anti-patterns.** Read the "Do's and Don'ts" of `.lila/DESIGN.md` and confirm your UI commits
   none of *that brand's* listed forbidden patterns/words.
3. **Real states + a11y floor.** Real empty/loading/error states, an SVG icon set (never emoji as
   icons), the type scale respected, and AA contrast on text.
4. Say in your summary which system is locked and that the screenshot adheres to it — backed by the
   grep result above and what you actually see in the image, not an adjective.
