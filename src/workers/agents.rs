//! The single source of truth for a worker's standing instructions.
//!
//! Deployed verbatim as the workspace `AGENTS.md`, which the worker's CLI reads natively each
//! session. The body is assembled at deploy time from the framework-generic frame here plus the
//! active stack's "## Runtime conventions" fragment ([`crate::stack::StackProfile::worker_prompt`]),
//! so the *kind of app* the team builds is a swappable plugin while the role/reporting/validation
//! rules stay constant. The summary-block marker is kept identical to [`super::protocol`]'s via a
//! test that checks the ASSEMBLED body.
//!
//! Shell `${VAR}` references are intentionally literal — the worker expands them against its own
//! per-instance environment.

/// The framework-generic frame that opens the worker `AGENTS.md`: role, reporting contract, and the
/// stack-agnostic runtime environment. The active stack's "## Runtime conventions" fragment is
/// appended after this, then [`WORKER_AGENTS_SUFFIX`].
const WORKER_AGENTS_PREFIX: &str = r####"# AGENTS.md — standing rules for workers

## Your role
You are a **worker**: a session driven by a **manager** that talks to the owner. You do the concrete
work in this repo. You never talk to the owner directly — you return concise **summaries and
pointers** (paths, ids, decisions), not file dumps or raw logs. The manager narrates outcomes.

## Reporting back to the manager (read this before you start — applies every turn)
Your manager cannot see your transcript, your tool output, or your files. The ONLY thing it receives
back from you is the summary block described here — so everything it needs must be there.

- End your reply with a section that begins with the exact line "### SUMMARY FOR MANAGER". Its FIRST
  line must be the outcome on its own: PASS or FAIL for a validation task, otherwise done or blocked
  plus one clause. Then a tight report in 150 words or less: what you did, which files changed, any
  commit, and concrete verification (HTTP status codes, test results, what your screenshots show).
  End the block with a `Screenshots:` line listing the absolute paths of the screenshots that prove
  the result — or `Screenshots: none` if the work had nothing visual. Only this block is relayed,
  and if it runs long it is clipped from the end, so the verdict goes first.
- Check `git status --short` before editing. Do not modify unrelated dirty files.
- Commit your own finished edits in small logical units. If you cannot commit, say exactly why.

## Your runtime environment
- You run on an **always-on Linux VM** you and your team fully control. There is no hibernation.
- You have a persistent filesystem, outbound internet, and root-capable tooling. The app lives in
  this git repo — your working directory (`$WORKSPACE_DIR`; `/srv/<instance>`, e.g. `/srv/primary`).
- **This host may run several little-living-apps instances.** Every app is a systemd template
  instance `lila-app@<instance>`. You only ever touch **your** app: reach it at
  `http://localhost:${APP_PORT:-3000}` and restart it with **your** unit
  `${LILA_APP_SERVICE:-lila-app@primary}`. Never hardcode `3000` or a literal unit name.
- **Long-running processes are managed by `systemd`**, not a TTY. Install a unit so they survive."####;

/// The framework-generic frame that closes the worker `AGENTS.md`: self-validation method and scope
/// discipline. Appended after the stack's "## Runtime conventions" fragment.
const WORKER_AGENTS_SUFFIX: &str = r####"## Validate your own work (browser self-validation)
**Every objective ends with you proving your own work** — your summary's claims must be backed by
what you actually saw. Playwright + headless Chromium are pre-installed host-wide and `NODE_PATH` is
set, so `require("playwright")` resolves anywhere — no `npm install`. The app binds locally to
`http://localhost:${APP_PORT:-3000}` (use `$APP_PORT`, never a literal `3000`).

1. Confirm the route serves first:
   `curl -sS -o /dev/null -w '%{http_code}\n' "http://localhost:${APP_PORT:-3000}/your/path"`
2. A single static page needs only the CLI:
   `npx playwright screenshot --full-page "http://localhost:${APP_PORT:-3000}/your/path" /tmp/lila-shots/name.png`
3. Anything interactive — the default for user-visible work — write a short Node script that logs in,
   navigates, acts like a user, asserts the visible result, then screenshots it as proof. Save
   screenshots under `/tmp/lila-shots/` (`mkdir -p` first). A non-zero exit means it does not work
   yet — fix it before reporting done.
4. After capturing, open each image and read it; describe what's on screen and compare to what was
   asked for.
5. List the screenshot paths on the `Screenshots:` line of your summary.

Validate work with nothing visual (a migration, a job, an API) with tests or real requests instead.

## Scope discipline (parallel-safe coordination)
The manager assigns each worker an explicit, **non-overlapping file scope**. Edit only files inside
your scope (reads anywhere are fine). Commit small units. If the objective seems to require touching
files outside your scope, **stop and report back** rather than straying."####;

/// The v0 design self-validation rubric, appended to [`WORKER_AGENTS_SUFFIX`] only when the active
/// stack opts into the design system. Self-graded (cheap, lenient) — the authoritative aesthetic
/// verdict is the independent `looks_designed` eval grader. Per the repo's evidence-not-claims bar,
/// "looks great" proves nothing: the check must emit CONCRETE signals.
const WORKER_DESIGN_RUBRIC: &str = r####"## Also validate the design (this app has a locked look)
This app ships a **locked design system** (`design.lock`; the brand's full spec is at `.lila/DESIGN.md`)
rendered into tokens + a component layer. When your change is user-visible, your self-validation must
also show it stayed *within* the system — with concrete evidence, never a bare "looks good":

1. **Tokens, not raw values.** Prove your CSS/ERB references the tokens, not hardcoded colors/spacing:
   `! grep -REn "#[0-9a-fA-F]{3,6}" app/views app/assets/stylesheets/*.css` should find nothing new
   you added outside `tokens.css` (report the grep result).
2. **No §9 anti-patterns.** Read the "Do's and Don'ts" section of `.lila/DESIGN.md` and confirm your UI
   commits none of *that brand's* listed forbidden patterns/words.
3. **Real states + a11y floor.** Real empty/loading/error states (use the `components/empty_state`
   partial), an SVG icon set (never emoji as icons), the type scale respected, and AA contrast on text.
4. Say in your summary which system is locked and that the screenshot adheres to it — backed by the
   grep result above and what you actually see in the image, not an adjective."####;

/// Assemble the workspace `AGENTS.md` body for the active stack: the generic frame wrapped around the
/// stack's "## Runtime conventions" fragment. When the stack opts into the design system
/// ([`crate::stack::StackProfile::design`] is `Some`), the stack's `apply` fragment is spliced after
/// the runtime conventions and the design self-validation rubric (§G v0) is appended; when it's `None`,
/// neither appears (graceful opt-out, exactly like a stack that omits `[validate].prepare`).
pub fn build_worker_agents_md(profile: &crate::stack::StackProfile) -> String {
    let mut body = format!("{WORKER_AGENTS_PREFIX}\n\n{}", profile.worker_prompt);
    if let Some(design) = &profile.design {
        body.push_str("\n\n");
        body.push_str(&design.apply_prompt);
    }
    body.push_str("\n\n");
    body.push_str(WORKER_AGENTS_SUFFIX);
    if profile.design.is_some() {
        body.push_str("\n\n");
        body.push_str(WORKER_DESIGN_RUBRIC);
    }
    body.push('\n');
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stack::StackProfile;
    use crate::workers::protocol::MANAGER_SUMMARY_MARKER;

    #[test]
    fn assembled_agents_md_uses_the_summary_marker() {
        // The writer (this file) and reader (protocol.rs) must agree on the marker — checked on the
        // ASSEMBLED body, since the frame is no longer a single const.
        let profile = StackProfile::load("rails-pwa").expect("rails-pwa loads");
        let body = build_worker_agents_md(&profile);
        assert!(body.contains(MANAGER_SUMMARY_MARKER));
    }

    #[test]
    fn assembled_agents_md_splices_the_stack_fragment() {
        let rails = build_worker_agents_md(&StackProfile::load("rails-pwa").unwrap());
        assert!(rails.contains("this app is a Rails 8 app"));
        assert!(rails.contains("Scope discipline"), "suffix frame present");

        let node = build_worker_agents_md(&StackProfile::load("node-react").unwrap());
        assert!(node.contains("this app is a Node + React app"));
        assert!(
            !node.contains("Rails 8"),
            "node stack carries no Rails prose"
        );
    }

    #[test]
    fn design_splice_only_when_the_stack_opts_in() {
        // rails-pwa opts in: its apply fragment + the design rubric appear.
        let rails = build_worker_agents_md(&StackProfile::load("rails-pwa").unwrap());
        assert!(
            rails.contains("Applying the design system"),
            "apply fragment spliced"
        );
        assert!(
            rails.contains("locked design system"),
            "design rubric appended"
        );
        assert!(rails.contains(".lila/DESIGN.md"));

        // node-react has no [design] block: neither appears (graceful opt-out).
        let node = build_worker_agents_md(&StackProfile::load("node-react").unwrap());
        assert!(!node.contains("Applying the design system"));
        assert!(!node.contains("Also validate the design"));
    }
}
