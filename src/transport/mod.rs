//! Outbound-only Telegram transport: a thin Bot API client, the long-poll loop, and owner delivery.

pub mod deliver;
pub mod poller;
pub mod telegram;

pub use telegram::{TelegramClient, TelegramMessage, TelegramUpdate};
