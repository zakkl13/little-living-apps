//! The owner delivery channel: manager text + optional image attachments → Telegram. Port of
//! `src/transport/deliver.ts`. This is the host-side half of the ATTACH contract: the driver strips
//! `ATTACH: /path` lines out of the reply; here we validate each path against disk (the manager can
//! only NAME paths) and upload the survivors, dropping a hallucinated/missing/non-image file with a
//! visible note rather than failing the turn.

use std::path::Path;

use super::telegram::TelegramClient;

/// Telegram caps bot photo uploads at 10 MB.
const MAX_PHOTO_BYTES: u64 = 10 * 1024 * 1024;
/// Only ever attach images — the channel is proof-of-work screenshots, not file export.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// Deliver `text` plus any valid `attachments` (as photos) to `chat_id`.
pub async fn deliver(
    client: &TelegramClient,
    chat_id: i64,
    text: &str,
    attachments: &[String],
) -> anyhow::Result<()> {
    let (photos, notes) = collect_photos(attachments);
    let body = compose_body(text, &notes);
    if !body.is_empty() {
        client.send_message(chat_id, &body).await?;
    }
    for (bytes, name) in photos {
        client.send_photo(chat_id, bytes, &name, None).await?;
    }
    Ok(())
}

/// Validate + load each attachment, returning the uploadable photos and notes about dropped ones.
fn collect_photos(attachments: &[String]) -> (Vec<(Vec<u8>, String)>, Vec<String>) {
    let mut photos = Vec::new();
    let mut notes = Vec::new();
    for path in attachments {
        match load_photo(path) {
            Ok(photo) => photos.push(photo),
            Err(err) => {
                tracing::warn!(%path, %err, "dropping undeliverable attachment");
                notes.push(format!("⚠️ (couldn't attach {})", file_name(path)));
            }
        }
    }
    (photos, notes)
}

/// The owner-visible message body: the manager text plus any drop notes.
fn compose_body(text: &str, notes: &[String]) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if !text.is_empty() {
        parts.push(text);
    }
    parts.extend(notes.iter().map(String::as_str));
    parts.join("\n")
}

/// Validate + read an attachment, returning (bytes, filename) or an error explaining why it dropped.
fn load_photo(path: &str) -> anyhow::Result<(Vec<u8>, String)> {
    let p = Path::new(path);
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if !IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        anyhow::bail!("not an image file");
    }
    let meta = std::fs::metadata(p)?;
    if meta.len() > MAX_PHOTO_BYTES {
        anyhow::bail!("too large for Telegram ({} bytes)", meta.len());
    }
    Ok((std::fs::read(p)?, file_name(path)))
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}
