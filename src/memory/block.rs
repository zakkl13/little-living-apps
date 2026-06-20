//! Block model for memory files. Port of `src/memory/block.ts`.
//!
//! Every memory file may carry frontmatter with `description` (always visible to the manager even
//! when the body isn't) and an optional `limit`. We hand-roll a tiny parser rather than pull in a
//! YAML dependency — only flat `key: value` scalars are supported.

const FENCE: &str = "---";

/// Parsed memory file: optional metadata + body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryBlock {
    /// One-line summary shown in the tree even when the body is not loaded.
    pub description: Option<String>,
    /// Optional character budget for the body (advisory).
    pub limit: Option<i64>,
    /// Everything after the frontmatter.
    pub body: String,
}

/// Parse a raw file into a [`MemoryBlock`]. No (or unterminated) frontmatter → whole file is body.
pub fn parse_block(raw: &str) -> MemoryBlock {
    let normalized = raw.replace("\r\n", "\n");
    let fence_open = format!("{FENCE}\n");
    if !normalized.starts_with(&fence_open) {
        return MemoryBlock {
            body: normalized,
            ..Default::default()
        };
    }
    // Find the closing fence ("\n---") after the opening one.
    let Some(end) = normalized[FENCE.len()..]
        .find(&format!("\n{FENCE}"))
        .map(|i| i + FENCE.len())
    else {
        return MemoryBlock {
            body: normalized,
            ..Default::default()
        };
    };
    let frontmatter = &normalized[FENCE.len() + 1..end];
    // Body starts after the line containing the closing fence.
    let body = match normalized[end + 1..].find('\n') {
        Some(nl) => normalized[end + 1 + nl + 1..].to_string(),
        None => String::new(),
    };

    let mut block = MemoryBlock {
        body,
        ..Default::default()
    };
    for line in frontmatter.split('\n') {
        if let Some((key, value)) = parse_frontmatter_line(line.trim()) {
            match key.as_str() {
                "description" => block.description = Some(value),
                "limit" => block.limit = value.parse::<i64>().ok(),
                _ => {}
            }
        }
    }
    block
}

/// Parse a `key: value` frontmatter line (key starts with a letter/underscore). Returns the key and
/// the de-quoted value.
fn parse_frontmatter_line(line: &str) -> Option<(String, String)> {
    let (key, value) = line.split_once(':')?;
    let key = key.trim();
    if key.is_empty()
        || !key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    {
        return None;
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some((key.to_string(), strip_quotes(value.trim()).to_string()))
}

/// Serialize a block to disk, emitting frontmatter only when there is metadata to record.
pub fn serialize_block(block: &MemoryBlock) -> String {
    if block.description.is_none() && block.limit.is_none() {
        return block.body.clone();
    }
    let mut out = String::from(FENCE);
    out.push('\n');
    if let Some(desc) = &block.description {
        out.push_str(&format!("description: {desc}\n"));
    }
    if let Some(limit) = block.limit {
        out.push_str(&format!("limit: {limit}\n"));
    }
    out.push_str(FENCE);
    out.push('\n');
    out.push_str(&block.body);
    out
}

/// The text we index for search: description + body, so a file is findable by either.
pub fn indexable_text(block: &MemoryBlock) -> String {
    let mut parts = Vec::new();
    if let Some(desc) = &block.description
        && !desc.is_empty()
    {
        parts.push(desc.clone());
    }
    if !block.body.is_empty() {
        parts.push(block.body.clone());
    }
    parts.join("\n")
}

fn strip_quotes(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &s[1..s.len() - 1];
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter() {
        let b = parse_block("---\ndescription: hi there\nlimit: 200\n---\nbody line\n");
        assert_eq!(b.description.as_deref(), Some("hi there"));
        assert_eq!(b.limit, Some(200));
        assert_eq!(b.body, "body line\n");
    }

    #[test]
    fn no_frontmatter_is_all_body() {
        let b = parse_block("just a body\nsecond line");
        assert!(b.description.is_none());
        assert_eq!(b.body, "just a body\nsecond line");
    }

    #[test]
    fn roundtrips() {
        let b = MemoryBlock {
            description: Some("d".into()),
            limit: None,
            body: "the body\n".into(),
        };
        let s = serialize_block(&b);
        assert_eq!(parse_block(&s), b);
    }

    #[test]
    fn indexable_includes_description_and_body() {
        let b = parse_block("---\ndescription: findme\n---\nand the body too\n");
        let idx = indexable_text(&b);
        assert!(idx.contains("findme") && idx.contains("body"));
    }
}
