//! Minimal reqwest-based Telegram Bot API client. Port of `src/transport/telegram.ts`. Deliberately
//! tiny so the base URL can be pointed at a fake server in tests. Handles the one hard requirement:
//! responses > 4096 chars are chunked, never truncated.

use serde::Deserialize;

/// Telegram's per-message text cap.
pub const TELEGRAM_MAX_MESSAGE_LENGTH: usize = 4096;
/// Telegram's photo-caption cap.
pub const TELEGRAM_MAX_CAPTION_LENGTH: usize = 1024;

/// One size of an inbound photo. Telegram orders the array ascending, so the last is the largest.
#[derive(Debug, Clone, Deserialize)]
pub struct TelegramPhotoSize {
    pub file_id: String,
}

/// An inbound message.
#[derive(Debug, Clone, Deserialize)]
pub struct TelegramMessage {
    #[serde(default)]
    pub text: Option<String>,
    /// Caption accompanying a photo (used as the turn's text).
    #[serde(default)]
    pub caption: Option<String>,
    /// Photo sizes when the owner sends an image.
    #[serde(default)]
    pub photo: Option<Vec<TelegramPhotoSize>>,
    pub chat: TelegramChat,
    #[serde(default)]
    pub from: Option<TelegramUser>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramChat {
    pub id: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramUser {
    pub id: i64,
}

/// An update from `getUpdates`.
#[derive(Debug, Clone, Deserialize)]
pub struct TelegramUpdate {
    #[serde(default)]
    pub update_id: Option<i64>,
    #[serde(default)]
    pub message: Option<TelegramMessage>,
    #[serde(default)]
    pub edited_message: Option<TelegramMessage>,
}

impl TelegramUpdate {
    /// The message carried (new or edited).
    pub fn message(&self) -> Option<&TelegramMessage> {
        self.message.as_ref().or(self.edited_message.as_ref())
    }
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "T: serde::de::DeserializeOwned"))]
struct ApiResponse<T> {
    ok: bool,
    #[serde(default)]
    result: Option<T>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct MessageId {
    message_id: i64,
}

#[derive(Deserialize)]
struct FilePath {
    #[serde(default)]
    file_path: Option<String>,
}

/// A thin Bot API client.
#[derive(Debug, Clone)]
pub struct TelegramClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl TelegramClient {
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: token.into(),
            http: reqwest::Client::new(),
        }
    }

    fn api(&self, method: &str) -> String {
        format!("{}/bot{}/{}", self.base_url, self.token, method)
    }

    async fn call<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        body: serde_json::Value,
    ) -> anyhow::Result<Option<T>> {
        let res = self.http.post(self.api(method)).json(&body).send().await?;
        let status = res.status();
        let parsed: ApiResponse<T> = res.json().await?;
        if !status.is_success() || !parsed.ok {
            anyhow::bail!(
                "Telegram {method} failed: {status} {}",
                parsed.description.unwrap_or_default()
            );
        }
        Ok(parsed.result)
    }

    /// Send text, chunked at 4096. Returns the first chunk's message id.
    pub async fn send_message(&self, chat_id: i64, text: &str) -> anyhow::Result<Option<i64>> {
        let mut parts = chunk_text(text, TELEGRAM_MAX_MESSAGE_LENGTH);
        if parts.is_empty() {
            parts.push("(empty response)".to_string());
        }
        let mut first = None;
        for part in parts {
            let msg: Option<MessageId> = self
                .call(
                    "sendMessage",
                    serde_json::json!({ "chat_id": chat_id, "text": part }),
                )
                .await?;
            if first.is_none() {
                first = msg.map(|m| m.message_id);
            }
        }
        Ok(first)
    }

    /// Upload an image (multipart sendPhoto). Caption clipped to Telegram's cap.
    pub async fn send_photo(
        &self,
        chat_id: i64,
        bytes: Vec<u8>,
        filename: &str,
        caption: Option<&str>,
    ) -> anyhow::Result<Option<i64>> {
        let part = reqwest::multipart::Part::bytes(bytes).file_name(filename.to_string());
        let mut form = reqwest::multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .part("photo", part);
        if let Some(cap) = caption {
            let clipped: String = cap.chars().take(TELEGRAM_MAX_CAPTION_LENGTH).collect();
            form = form.text("caption", clipped);
        }
        let res = self
            .http
            .post(self.api("sendPhoto"))
            .multipart(form)
            .send()
            .await?;
        let status = res.status();
        let parsed: ApiResponse<MessageId> = res.json().await?;
        if !status.is_success() || !parsed.ok {
            anyhow::bail!(
                "Telegram sendPhoto failed: {status} {}",
                parsed.description.unwrap_or_default()
            );
        }
        Ok(parsed.result.map(|m| m.message_id))
    }

    /// Clear any registered webhook (mutually exclusive with getUpdates).
    pub async fn delete_webhook(&self) -> anyhow::Result<()> {
        let _: Option<bool> = self
            .call(
                "deleteWebhook",
                serde_json::json!({ "drop_pending_updates": false }),
            )
            .await?;
        Ok(())
    }

    /// Long-poll for new updates.
    pub async fn get_updates(
        &self,
        offset: Option<i64>,
        timeout_secs: u32,
    ) -> anyhow::Result<Vec<TelegramUpdate>> {
        let mut body =
            serde_json::json!({ "timeout": timeout_secs, "allowed_updates": ["message"] });
        if let Some(off) = offset {
            body["offset"] = off.into();
        }
        Ok(self.call("getUpdates", body).await?.unwrap_or_default())
    }

    /// Resolve a `file_id` to a server-side file path.
    pub async fn get_file(&self, file_id: &str) -> anyhow::Result<Option<String>> {
        let fp: Option<FilePath> = self
            .call("getFile", serde_json::json!({ "file_id": file_id }))
            .await?;
        Ok(fp.and_then(|f| f.file_path))
    }

    /// Download a file (by its getFile path) as raw bytes.
    pub async fn download_file(&self, file_path: &str) -> anyhow::Result<Vec<u8>> {
        let url = format!("{}/file/bot{}/{}", self.base_url, self.token, file_path);
        let res = self.http.get(url).send().await?;
        if !res.status().is_success() {
            anyhow::bail!("Telegram file download failed: {}", res.status());
        }
        Ok(res.bytes().await?.to_vec())
    }
}

/// Split text into Telegram-sized chunks (≤ `max`), preferring newline boundaries.
pub fn chunk_text(text: &str, max: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut start = 0;
    while chars.len() - start > max {
        let window_end = start + max;
        // Prefer breaking on the last newline in the window, if it leaves a reasonably full chunk;
        // otherwise hard-cut at the window edge.
        let cut = chars[start..window_end]
            .iter()
            .rposition(|&c| c == '\n')
            .map(|i| start + i)
            .filter(|&c| c - start >= max / 2)
            .unwrap_or(window_end);
        chunks.push(chars[start..cut].iter().collect());
        start = cut;
        if chars.get(start) == Some(&'\n') {
            start += 1; // drop the leading newline of the next chunk
        }
    }
    if start < chars.len() {
        chunks.push(chars[start..].iter().collect());
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_one_chunk() {
        assert_eq!(chunk_text("hello", 4096), vec!["hello".to_string()]);
        assert!(chunk_text("", 4096).is_empty());
    }

    #[test]
    fn long_text_chunks_on_newline() {
        let line = "x".repeat(50);
        let text = format!("{line}\n{line}\n{line}");
        let chunks = chunk_text(&text, 60);
        assert!(chunks.len() >= 2);
        assert!(chunks.iter().all(|c| c.chars().count() <= 60));
    }

    #[test]
    fn hard_cuts_when_no_newline() {
        let text = "y".repeat(200);
        let chunks = chunk_text(&text, 80);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.chars().count() <= 80));
    }
}
