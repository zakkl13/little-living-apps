# Defects

Format: `[ID] (area) severity — symptom → suspected fix location`. Status: OPEN / FIXED.

> **Reset at the host-native migration.** The original log tracked live validation of the Fly
> Sprite substrate (Sprite CLI, single-public-port routing, webhook auth, keep-alive Tasks API).
> That substrate has been removed (see `MIGRATION.md`), so those entries — D1–D10, D13–D16, and the
> Sprite/webhook/keep-alive phase logs — are obsolete and were dropped. Only substrate-neutral
> findings carry forward. New host-native defects go below.

## Open

- **[D12] (observability) MED — OPEN.** Key turn-lifecycle events log at `debug`, not `info`
  (`logger.ts` threshold). A complete successful turn can produce zero log lines beyond startup, so
  in production a failure may leave little trace. Promote turn start/end, worker start/settle, and
  errors to `info`, or document running with `LOG_LEVEL=debug`. → `src/**` log call sites,
  `src/logger.ts`.

## Resolved (kept for context)

- **[D11] — RESOLVED.** Haiku utility model rejected the `compact_20260112` context-management
  strategy (400). Removed when the summarizer became a no-model hard clip (`clipSummarizer`); the
  utility model tier is gone.
- **[D13] — RESOLVED.** Manager replies double-delivered (the `notify_user` tool *and* the
  end-turn text fallback both fired). Resolved by removing `notify_user` entirely: the manager's
  plain `text` is the single reply channel (Hermes/Letta-v1 style).
- **[D14] — RESOLVED.** `NO_REPLY` reasoning leak: the model emitted private reasoning plus the
  sentinel in one text block. Root cause — the Anthropic wrapper never set the `thinking` param, so
  the model had no private channel. Now sets adaptive thinking; reasoning lands in `thinking`
  blocks (never delivered). Whole-message suppression on a lone `NO_REPLY` line remains as defense.
