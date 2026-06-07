# AGENTS.md — how to build durable apps on this Sprite

This file is loaded automatically into every Codex session running in this workspace. It
encodes the Sprite-correct patterns so everything you build is durable by default.

## Your runtime environment
- You run on a Fly **Sprite**: a persistent Linux microVM that **hibernates after ~30s idle**
  and **wakes on demand** (on an HTTP request to its URL, or an external ping). Treat sleep/wake as constant and normal.
- You have a **100 GB persistent filesystem** and outbound internet. CPU/RAM exist only while awake.

## The non-negotiable rules
1. **RAM is wiped on hibernation. The filesystem is not.** Never store anything that must
   survive in memory. Persist all durable state to disk under `/workspace` (SQLite or files).
   Assume the process can be killed and restarted at any moment.
2. **Any long-running process MUST be a Sprite Service.** Web servers, workers, and listeners must be registered as Services so they auto-restart on wake. **Never** leave a server running only inside a console/TTY session — it dies the moment the Sprite sleeps. *(Use the Sprite Services mechanism; verify the exact command in docs.sprites.dev.)*
3. **Web servers must listen on the Sprite's routed HTTP port** (e.g. 8080) so an incoming
   request wakes the Sprite and the Service answers it.
4. **There is no built-in cron.** For anything scheduled (periodic scraping, digests, cleanups), do **not** rely on `cron` or `systemd` timers firing while asleep. Expose a job endpoint and rely on the external heartbeat scheduler to wake the Sprite and trigger it. Make every scheduled job **idempotent** — it may fire late, twice, or after a long sleep.
5. **Write for restart-tolerance.** Re-entrant init, reconnect logic, resume-from-disk. The box sleeps and wakes constantly; code that assumes continuous uptime will break.

## Working discipline
6. **Always work inside the git repo** at `/workspace/project`. Commit in small, logical units
   with clear messages — this is the rollback mechanism. Before destructive or risky changes,
   prefer creating a checkpoint (and note that a full-filesystem snapshot is available as a
   coarser undo).
7. **Leave breadcrumbs.** When you finish a task, update a short `STATUS.md` (what exists, what's
   running as a Service, what each scheduled job does, known issues). After hibernation there is
   no memory of this session except what's on disk — write for the next cold start.

## Security & safety
8. **Treat the Sprite as the isolation boundary.** Stay within `/workspace`. Don't attempt to
   exfiltrate credentials or escape the box. Minimize destructive operations.
9. **Never hardcode or commit secrets.** Read credentials from environment variables / secret
   files only. Don't echo secrets into logs or `STATUS.md`.
10. **Default to private.** Only make a port or URL public when the task explicitly requires it
    (e.g. a webhook). For any public endpoint, require a shared secret and verify it on every
    request.

## Resource awareness
11. You share one Sprite (100 GB disk; CPU/RAM only while awake). Keep dependencies lean, clean
    up build artifacts, and avoid unbounded disk growth (rotate logs, cap caches).
