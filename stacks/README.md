# Stacks — the *kind of app* the team builds is a plugin

A **stack** decides what kind of app little-living-apps scaffolds, serves, and maintains: Rails 8 +
PWA by default, a zero-build Node + React PWA alongside it, or anything you add. A stack is **data,
not code** — a directory under `stacks/`, read by the generic framework. Adding one needs **no Rust
changes and no recompile**.

Pick the active stack per instance with `LILA_STACK` (default `rails-pwa`), set in `.env` or
`.docker/<instance>.env`. The same value drives six things from one source of truth: the scaffold,
the serve command, the app toolchain, the manager prompt, the worker prompt, and the eval fixture.

## Layout

```
stacks/
  <name>/
    stack.toml        # the contract (below)
    scaffold.sh       # creates the app at instance stand-up (runtime, via lila-new-app)
    worker.md         # the "## Runtime conventions" fragment spliced into the worker AGENTS.md
    manager.md        # the "the app" fragment spliced into the manager's runtime-environment section
    eval/
      fixture/        # a pre-scaffolded copy of the app, cloned per eval trial (APFS clone)
      setup.sh        # optional one-time build of the fixture (e.g. vendoring deps) — see rails-pwa
```

## The contract — `stack.toml`

```toml
name    = "rails-pwa"          # must match the directory name
display = "Rails 8 + PWA"      # human-readable label

# App-language toolchain pins documented by the stack contract. The Docker image owns the installed
# runtime set.
# Omit the section entirely for a Node-only stack.
[toolchain]
ruby = "3.3"

# The scaffold script, run at instance creation by lila-new-app in the app dir, with LILA_INSTANCE /
# APP_DIR / APP_PORT / LILA_DOMAIN / SKIP_AUTH / SERVICE_USER / MISE in the environment. Full bash:
# conditionals, post-tweaks, idempotency guards.
[scaffold]
script = "scaffold.sh"

# How to start the app. Portable command: it reads ${APP_HOST} and ${APP_PORT} from the environment.
# Docker runs it directly after lila-new-app; the eval probe runs it directly. `env` is exported by
# each runner.
[serve]
exec = "bin/rails server -b ${APP_HOST} -p ${APP_PORT}"
env  = { RAILS_ENV = "development" }

# Validation contract: the app's own test command, a route it serves once booted (the eval probe
# waits on it before probing), and an optional failure-tolerant pre-boot step the eval probe runs.
[validate]
test_cmd    = "bin/rails test"
health_path = "/up"
prepare     = "bin/rails db:prepare"   # optional; omit if the app needs no preparation to boot

# Prose fragments spliced verbatim into the agent prompts — the only stack-specific text. Design
# guidance, if a stack renders UI, lives in worker.md as prose (see "Design" below); it is NOT part of
# the stack contract.
[prompt]
worker  = "worker.md"
manager = "manager.md"
```

`node-react/stack.toml` is the same shape with no `[toolchain]`, `exec = "node server.js"`,
`test_cmd = "node --test"`, `health_path = "/"`, and no `prepare`.

## The prompt fragments

- **`worker.md`** is the `## Runtime conventions (this app is a …)` section. The framework wraps it
  with the constant role / reporting / browser-self-validation / scope rules to form the workspace
  `AGENTS.md` (and `CLAUDE.md`). Use `$LILA_APP_URL`, `$APP_PORT`, and `$LILA_APP_RESTART_CMD`
  literally — the worker expands them against its own per-instance environment.
- **`manager.md`** is the "the app" bullets in the manager's runtime-environment section (what kind of
  app it is, how it reloads). Use the placeholders `{workspace}` and `{restart_cmd}`; the framework
  fills them from the live runtime facts.

## Design — framework-generic, not part of the stack contract

The design system is **orthogonal to the stack**: a design system is an abstract, stack-neutral bundle
("Linear-ish" reads the same in Rails or a SPA), so it is not a dimension of `stack.toml`. The
**catalog** lives once at `design/systems/<brand>/DESIGN.md` (vendored from
[Open Design](https://github.com/nexu-io/open-design); see `design/systems/PROVENANCE`). Membership in
three nested pools — `default` (the ~3 safe neutrals the framework may draw blindly) ⊂ `browsable` (the
curated slice offered on request) ⊂ `full` (all 150, reachable only by an explicit pin) — is recorded in
`design/systems/INDEX.md`.

The active choice is `LILA_DESIGN` (default `random`): `random` (blind draw from the **default** pool),
`random:<seed>` (reproducible), or `<brand>` (pin any system from any pool). At standup `lila-new-app`
**always** resolves it with `lila design draw` and passes the chosen system's package dir into the
scaffold env. A stack that renders UI consumes it in its own `scaffold.sh` — `rails-pwa` **installs the
curated baseline** (it does NOT re-derive tokens or ship a hand-written component layer):

- upstream's machine-readable **`tokens.css`** is copied verbatim into the app's token sink
  (`app/assets/stylesheets/tokens.css` for `rails-pwa`; agents reference it via `var(--name)`);
- the rest of the curated package — `DESIGN.md`, `USAGE.md`, `components.html`, `components.manifest.json`,
  `design-tokens.json` — is copied into the app's **`.lila/`** as the agent's reference; the worker
  **adapts** those reference components into the stack's idiom (ERB for `rails-pwa`) per the system's own
  `USAGE.md`, rather than inheriting a pre-built component layer;
- the scaffold writes a committed **`design.lock`** at the app root (the active brand + the
  selection-flow `source`: `default` | `invited` | `chosen` | `pinned`). The look is **locked for the
  app's life** — the scaffold never rerolls an existing lock; only a user-driven selection rewrites it.

A stack whose `scaffold.sh` ignores the draw simply renders no tokens and writes no `design.lock`, and
every design consumer falls quiet on its own: the worker's design guidance is just prose in its
`worker.md`, the manager's design-flow policy never fires without a `design.lock` to read, and the
`looks_designed` eval grader only runs on the design scenarios (pinned to `rails-pwa`).

## Eval fixture

Each stack ships `eval/fixture/` — a committed, pre-scaffolded copy of the app the eval suite operates
on. The harness clones it per trial, writes the assembled worker rules, applies the scenario's
planted-bug overlay, and grades the real end state with the profile-driven graders (`test_cmd`,
`serve.exec` + `health_path`). If the fixture needs a one-time build (Rails vendors its gems into
`vendor/bundle`), put it in `eval/setup.sh` and run it once before the eval. The planted realities are
proven by `cargo test` (`tests/eval_graders.rs`, `tests/eval_rails.rs`).

## Add a stack in four steps

1. `mkdir stacks/<name>` and write `stack.toml`, `scaffold.sh`, `worker.md`, `manager.md`.
2. Add `eval/fixture/` (a minimal working app) so the suite can run on it — and `eval/setup.sh` if it
   needs a one-time build.
3. Smoke-test the contract: `lila stack <name>` should print the `LILA_STACK_*` assignments without
   error, and `cargo test` (the stack-loader unit tests parse every in-repo stack).
4. Run it: set `LILA_STACK=<name>` and start an instance with `bin/new-instance`. No Rust
   edits, no rebuild.
