//! Telegram long-poll transport. Port of `src/transport/poller.ts`. The box makes OUTBOUND
//! getUpdates calls and never listens on a port. One loop pulls updates, forwards each over a
//! channel (the app authorizes + enqueues), and advances the confirmation offset past it.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedSender;

use super::telegram::{TelegramClient, TelegramUpdate};

/// Default long-poll timeout (Telegram holds the request open this long when idle).
const DEFAULT_TIMEOUT_SECS: u32 = 50;
/// Backoff after a failed poll.
const BACKOFF: Duration = Duration::from_millis(1000);

/// Run the poll loop until `shutdown` is notified. Each update is forwarded to `tx`.
pub async fn run(
    client: TelegramClient,
    tx: UnboundedSender<TelegramUpdate>,
    shutdown: Arc<Notify>,
) {
    let mut offset: Option<i64> = None;
    tracing::info!(timeout = DEFAULT_TIMEOUT_SECS, "Telegram long-poll started");
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            res = client.get_updates(offset, DEFAULT_TIMEOUT_SECS) => match res {
                Ok(updates) => {
                    for update in updates {
                        if let Some(id) = update.update_id {
                            offset = Some(id + 1);
                        }
                        if tx.send(update).is_err() {
                            return; // app loop gone — stop polling
                        }
                    }
                }
                Err(err) => {
                    tracing::error!(%err, "getUpdates failed; backing off");
                    tokio::select! {
                        _ = shutdown.notified() => break,
                        _ = tokio::time::sleep(BACKOFF) => {}
                    }
                }
            }
        }
    }
    tracing::info!("Telegram long-poll stopped");
}
