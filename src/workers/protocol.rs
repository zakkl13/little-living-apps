//! The reader half of the worker⇄manager contract. The writer
//! half (standing rules telling a worker to end with a summary block) lives in `agents.rs`; they are
//! coupled through [`MANAGER_SUMMARY_MARKER`] so they can never drift.

/// The exact marker a worker writes before its manager-facing summary block.
pub const MANAGER_SUMMARY_MARKER: &str = "### SUMMARY FOR MANAGER";

/// Safety ceiling if a worker over-writes its summary block.
const SUMMARY_CEILING: usize = 1500;

/// Pull just the manager-summary block out of a worker's full output. Falls back to the TAIL (where
/// the conclusion lives) when no block is present — never the head, which is setup/preamble.
pub fn extract_manager_summary(output: &str) -> String {
    if let Some(idx) = output.rfind(MANAGER_SUMMARY_MARKER) {
        let block = output[idx + MANAGER_SUMMARY_MARKER.len()..].trim();
        if block.chars().count() <= SUMMARY_CEILING {
            return block.to_string();
        }
        // Over-ceiling: keep BOTH ends (verdict-first head + conclusion tail).
        let head = take_chars(block, SUMMARY_CEILING - 400);
        let tail = take_last_chars(block, 380);
        return format!("{head}\n…(clipped)…\n{tail}");
    }
    let trimmed = output.trim();
    if trimmed.chars().count() <= SUMMARY_CEILING {
        return trimmed.to_string();
    }
    format!(
        "…(no summary block; showing the tail)\n{}",
        take_last_chars(trimmed, SUMMARY_CEILING)
    )
}

fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn take_last_chars(s: &str, n: usize) -> String {
    let total = s.chars().count();
    s.chars().skip(total.saturating_sub(n)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_block_after_marker() {
        let out = "lots of setup\n### SUMMARY FOR MANAGER\nPASS — built the thing.";
        assert_eq!(extract_manager_summary(out), "PASS — built the thing.");
    }

    #[test]
    fn falls_back_to_tail_without_marker() {
        let out = "no marker here, just a conclusion at the end";
        assert_eq!(extract_manager_summary(out), out);
    }

    #[test]
    fn uses_last_marker_occurrence() {
        let out = "### SUMMARY FOR MANAGER\nfirst\n### SUMMARY FOR MANAGER\nFAIL — real one";
        assert_eq!(extract_manager_summary(out), "FAIL — real one");
    }
}
