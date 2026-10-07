//! Stale-jump retry policy.
//!
//! Live panes keep printing, so Herdr rejects a `pane.copy_mode_jump` whose captured revision or
//! scroll identity no longer matches. That rejection is right in general, but a target is still the
//! target when the only thing that changed is unrelated output: the picker re-reads the pane and
//! re-issues the jump against the fresh identity when the picked row still holds the same text.

use crate::buffer::Buffer;

/// True when the error is Herdr telling us the capture no longer matches the pane.
pub fn is_stale(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    message.contains("stale_content") || message.contains("stale_pane_viewport")
}

/// Whether the jump may be re-issued against a freshly read pane.
///
/// Only the picked row matters: if it still holds the text the user picked from, the cell is still
/// the cell they meant. The fresh capture is re-wrapped exactly like the picker's own grid, so the
/// comparison stays in viewport rows even when the row is a soft-wrapped segment. Anything else
/// (the row changed, the row is gone) keeps the rejection.
pub fn may_retry(expected_row: &str, fresh: &str, row: u32, wrap_width: Option<usize>) -> bool {
    let Ok(row) = usize::try_from(row) else {
        return false;
    };
    Buffer::from_text(fresh, wrap_width).row_text(row) == expected_row
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use pretty_assertions::assert_eq;

    #[test]
    fn stale_errors_are_recognized() {
        assert!(is_stale(&anyhow!(
            "Herdr API error stale_content: pane content changed"
        )));
        assert!(is_stale(&anyhow!(
            "Herdr API error stale_pane_viewport: pane viewport changed"
        )));
        assert!(!is_stale(&anyhow!("Herdr API error pane_not_found: nope")));
    }

    #[test]
    fn the_jump_retries_when_the_picked_row_is_unchanged() {
        let fresh = "prompt\nfoo example bar\nstatus: busy";
        // Only the status row churned: the target row still holds the same text.
        assert!(may_retry("foo example bar", fresh, 1, None));
    }

    #[test]
    fn the_jump_stays_rejected_when_the_picked_row_moved() {
        assert!(!may_retry(
            "foo example bar",
            "prompt\nother line\nfoo example bar",
            1,
            None
        ));
        assert!(!may_retry("foo example bar", "prompt", 1, None));
        // The row itself is gone past the end of the capture.
        assert!(!may_retry("foo example bar", "prompt", 9, None));
    }

    #[test]
    fn rows_are_compared_in_the_pickers_wrapped_grid() {
        // The picker re-wraps the capture at the pane width, so a soft-wrapped row is compared as
        // the segment the user picked from, not as the logical line.
        let fresh = "alpha beta gamma";
        assert_eq!(Buffer::from_text(fresh, Some(5)).row_text(1), " beta");
        assert!(may_retry("alpha", fresh, 0, Some(5)));
        assert!(may_retry(" beta", fresh, 1, Some(5)));
        assert!(!may_retry("alpha beta gamma", fresh, 0, Some(5)));
    }
}
