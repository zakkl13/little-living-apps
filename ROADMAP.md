# ROADMAP

Ideas under consideration for little-living-apps. Captured as problems/directions, not committed
designs — the "how" is deliberately left open.

## App observability

Give the apps the agent builds a first-class observability capability — the way an engineer
leans on Datadog and its dashboards to understand and maintain a running service.

Two halves:

1. **Instrument** — apps emit logs and metrics about their own runtime behavior (latency, error
   rates, unexpected/unhandled errors, key product flows).
2. **Review** — the agent can read that instrumentation to track and understand how the app is
   actually behaving in production, the way a human would scan an APM dashboard.

The point is to close the loop on "living": today the team is largely blind to runtime behavior
between objectives, so problems are invisible until the owner complains. With this, the app's own
behavior becomes something the agents can observe, react to, and maintain against.

> Note: distinct from the framework's own telemetry/Inspector, which observes the *agent system*
> for the human operator. This is about the *apps the agents build* observing themselves, for the
> agents.

## Heartbeat: the autonomous tick

Today the manager only wakes for an owner message. A **heartbeat** — a periodic self-trigger (cron)
that enqueues a "review" event — would let the manager take a turn with no human in the loop: scan
the app's logs and metrics (see *App observability*), notice regressions or slow paths, and dispatch
workers to fix or improve before the owner ever notices.

This is the move that turns the system from request/response into a true **flywheel**: the loop keeps
spinning on its own momentum, not just when poked. It rides the existing event queue — a heartbeat
event is just another item the serialized manager turn consumes — so durability and one-turn-at-a-time
ordering come for free. Open questions: cadence and backoff, how to keep idle ticks cheap, and how the
manager decides a tick is worth acting on versus staying silent (`NO_REPLY`).

## More spokes on the wheel: external feedback inputs

The heartbeat and owner messages are two inputs to the loop; the architecture invites more. Each is
just another event on the queue — another spoke feeding the flywheel:

- **Customer feedback** — an in-app feedback widget (or a `/feedback` route the scaffold ships) that
  posts straight onto the manager's queue, so real user reports drive the next change.
- **Alerts & webhooks** — error/exception hooks, uptime checks, or arbitrary inbound webhooks that
  wake the manager when the running app misbehaves.
- **Scheduled reviews** — a weekly "what should we improve?" tick that reads usage and proposes work.

None of these exist out of the box, but they all land through the same door. The design goal is one
queue, many triggers — adding an input should never mean touching the manager loop.

## Compounding loop: learn from production, not just from the turn

The inner loops (plan → build → verify) already work per objective. The outer, **hill-climbing** loop
is the opportunity: feed graded production behavior back into memory so each cycle makes the next one
better. Tie the eval rubric to live outcomes, capture recurring failure patterns as durable memory the
manager consults before planning, and let the app's own runtime behavior — not just the owner's words
— shape what gets built next. This is what makes the memory the flywheel's accumulating mass.

## Turn-lifecycle logging at `info`

Key turn-lifecycle events (turn start/end, worker start/settle, errors) currently log at `debug`,
below the default `tracing` threshold (`src/logging.rs`). A complete successful turn can produce zero
log lines beyond startup, so in production a failure may leave little trace. Promote those events to
`info` (or document running with `RUST_LOG=debug`) so the live system is observable by default.
→ the `tracing` subscriber threshold in `src/logging.rs` + the log call sites across `src/**`.
