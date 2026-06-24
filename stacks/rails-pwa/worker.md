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
- **Don't pick or swap the system yourself.** Which look is locked is the manager's call — it drives
  any owner-requested change through its `design` setting. Your two design jobs are: build *within* the
  locked look (above), and — only when the manager hands you a *staged* look change — fit it to the app
  (below).

## Fitting a staged design change (only when the manager hands you one)

The manager has already staged the new system: `.lila/` now holds its full curated package
(`tokens.css`, `DESIGN.md`, `USAGE.md`, `components.html`, `components.manifest.json`,
`design-tokens.json`) and `design.lock` already names it with `source = chosen`. You do **not** draw,
re-pick, or re-lock anything — you make the staged look real in the app:

1. **Install the new tokens.** Copy `.lila/tokens.css` verbatim into the app's token sheet
   (`app/assets/stylesheets/tokens.css` for this stack — already linked in the layout). Never hand-edit
   token values.
2. **Re-fit the views/components** to the new tokens + reference (`.lila/USAGE.md`,
   `.lila/components.html`). Token *names* are stable across systems, so most carries over; update any
   component CSS that baked in the old system's recipes, and honor the new brand's "Do's and Don'ts".
3. **Restart the app, then self-validate as below** (screenshot the changed screens + the token grep)
   and report which system is now live.

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
