//! Hint assignment: the picker labels at most [`MAX_HINTS`] matches per query.
//!
//! The label set is the first five keys of the configured alphabet (default `asdgh`, the head of
//! tmux-easy-motion's default hint alphabet). Candidates are ranked by distance from the buffer
//! anchor — the bottom of the visible screen, where the shell prompt and the copy-mode cursor
//! start — and labelled nearest-first, so `a` is always the closest hit (tmux-easymotion's
//! "closer matches get shorter hints").

use crate::buffer::{Buffer, Span};

/// The product limit: at most five hints are typable per query.
pub const MAX_HINTS: usize = 5;

/// Default label keys: the first five of tmux-easy-motion's `asdghklqwertyuiopzxcvbnmfj;`.
pub const DEFAULT_HINT_KEYS: &str = "asdgh";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hint {
    pub key: char,
    pub span: Span,
}

/// Deduplicate configured keys, drop whitespace, and cap the list at [`MAX_HINTS`].
pub fn sanitize_keys(configured: &str) -> Vec<char> {
    let mut keys: Vec<char> = Vec::new();
    for ch in configured.chars() {
        if ch.is_whitespace() || ch.is_control() || keys.contains(&ch) {
            continue;
        }
        keys.push(ch);
        if keys.len() == MAX_HINTS {
            break;
        }
    }
    keys
}

/// Label up to `keys.len()` matches, nearest to `anchor` first.
pub fn assign(buffer: &Buffer, matches: &[Span], anchor: (usize, u16), keys: &[char]) -> Vec<Hint> {
    if matches.is_empty() || keys.is_empty() {
        return Vec::new();
    }
    let anchor_index = buffer
        .cell_index_at(anchor.0, anchor.1)
        .or_else(|| buffer.row_head(anchor.0))
        .unwrap_or(0);
    let mut candidates = matches.to_vec();
    candidates.sort_by_key(|span| (distance(buffer, span.start, anchor_index), span.start));
    candidates.truncate(keys.len());
    candidates
        .into_iter()
        .zip(keys.iter().copied())
        .map(|(span, key)| Hint { key, span })
        .collect()
}

fn distance(buffer: &Buffer, left: usize, right: usize) -> (usize, usize) {
    let (left_row, left_col) = buffer.position_of(left);
    let (right_row, right_col) = buffer.position_of(right);
    (
        left_row.abs_diff(right_row),
        usize::from(left_col).abs_diff(usize::from(right_col)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::find_matches;
    use pretty_assertions::assert_eq;

    fn keys() -> Vec<char> {
        sanitize_keys(DEFAULT_HINT_KEYS)
    }

    #[test]
    fn default_keys_are_capped_at_five() {
        assert_eq!(keys(), ['a', 's', 'd', 'g', 'h']);
        assert_eq!(sanitize_keys("abcdefghij"), ['a', 'b', 'c', 'd', 'e']);
        assert_eq!(sanitize_keys("a a b"), ['a', 'b']);
        assert_eq!(sanitize_keys(""), Vec::<char>::new());
    }

    #[test]
    fn labels_are_typed_once_and_nearest_first() {
        let buffer = Buffer::from_text("one two three", None);
        let matches = find_matches(&buffer, "o");
        let hints = assign(&buffer, &matches, (0, 0), &keys());
        assert_eq!(
            hints
                .iter()
                .map(|hint| (hint.key, buffer.position_of(hint.span.start)))
                .collect::<Vec<_>>(),
            [('a', (0, 0)), ('s', (0, 6))]
        );
    }

    #[test]
    fn only_the_five_nearest_matches_get_hints() {
        let mut text = String::new();
        for index in 0..9 {
            text.push_str(&format!("hit{index} "));
        }
        let buffer = Buffer::from_text(&text, None);
        let matches = find_matches(&buffer, "hit");
        assert_eq!(matches.len(), 9);
        let hints = assign(&buffer, &matches, (0, 0), &keys());
        assert_eq!(hints.len(), MAX_HINTS);
        assert_eq!(hints.iter().map(|hint| hint.key).collect::<Vec<_>>(), keys());
        assert_eq!(
            hints
                .iter()
                .map(|hint| buffer.position_of(hint.span.start).1)
                .collect::<Vec<_>>(),
            [0, 5, 10, 15, 20]
        );
    }

    #[test]
    fn the_bottom_of_the_screen_is_the_anchor() {
        let buffer = Buffer::from_text("near\nfar\nnear", None);
        let matches = find_matches(&buffer, "near");
        let hints = assign(&buffer, &matches, (2, 0), &keys());
        assert_eq!(
            hints
                .iter()
                .map(|hint| buffer.position_of(hint.span.start).0)
                .collect::<Vec<_>>(),
            [2, 0]
        );
    }

    #[test]
    fn no_hints_without_matches_or_keys() {
        let buffer = Buffer::from_text("text", None);
        assert!(assign(&buffer, &[], (0, 0), &keys()).is_empty());
        let matches = find_matches(&buffer, "t");
        assert!(assign(&buffer, &matches, (0, 0), &[]).is_empty());
    }
}
