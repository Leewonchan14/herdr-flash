//! Literal, smart-case query matching over a captured buffer.
//!
//! Matching is per logical line: a query may span a soft wrap but never a hard newline, and a
//! query is never empty. Case sensitivity mirrors Herdr's own copy-mode search: any uppercase
//! character in the query makes the whole search case-sensitive.

use crate::buffer::{Buffer, Span};

pub fn is_case_sensitive(query: &str) -> bool {
    query.chars().any(char::is_uppercase)
}

/// Non-overlapping matches in document order.
pub fn find_matches(buffer: &Buffer, query: &str) -> Vec<Span> {
    if query.is_empty() {
        return Vec::new();
    }
    let needle: Vec<char> = query.chars().collect();
    let case_sensitive = is_case_sensitive(query);
    let cells = buffer.cells();
    let mut matches = Vec::new();
    for line in buffer.lines() {
        let line_cells = &cells[line.start..line.end];
        if line_cells.len() < needle.len() {
            continue;
        }
        let mut index = 0;
        while index + needle.len() <= line_cells.len() {
            let hit = line_cells[index..index + needle.len()]
                .iter()
                .zip(&needle)
                .all(|(cell, want)| chars_equal(cell.ch, *want, case_sensitive));
            if hit {
                let start = line.start + index;
                matches.push(Span {
                    start,
                    end: start + needle.len(),
                });
                index += needle.len();
            } else {
                index += 1;
            }
        }
    }
    matches
}

fn chars_equal(left: char, right: char, case_sensitive: bool) -> bool {
    if left == right {
        return true;
    }
    if case_sensitive {
        return false;
    }
    left.to_lowercase().eq(right.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn positions(buffer: &Buffer, matches: &[Span]) -> Vec<(usize, u16)> {
        matches
            .iter()
            .map(|span| buffer.position_of(span.start))
            .collect()
    }

    #[test]
    fn finds_every_literal_occurrence_left_to_right() {
        let buffer = Buffer::from_text("foo bar foo", None);
        let matches = find_matches(&buffer, "foo");
        assert_eq!(positions(&buffer, &matches), [(0, 0), (0, 8)]);
    }

    #[test]
    fn lowercase_queries_match_case_insensitively() {
        let buffer = Buffer::from_text("Hello hello HELLO", None);
        assert_eq!(find_matches(&buffer, "hello").len(), 3);
    }

    #[test]
    fn uppercase_queries_are_case_sensitive() {
        let buffer = Buffer::from_text("Hello hello HELLO", None);
        assert_eq!(find_matches(&buffer, "Hello").len(), 1);
        assert_eq!(find_matches(&buffer, "HE").len(), 1);
    }

    #[test]
    fn a_query_can_span_a_soft_wrap_but_not_a_hard_line() {
        let buffer = Buffer::from_text("abcde\nfghij", Some(3));
        let matches = find_matches(&buffer, "cde");
        assert_eq!(matches.len(), 1);
        assert_eq!(buffer.position_of(matches[0].start), (0, 2));
        assert_eq!(find_matches(&buffer, "def").len(), 0);
    }

    #[test]
    fn matches_do_not_overlap() {
        let buffer = Buffer::from_text("aaaa", None);
        assert_eq!(find_matches(&buffer, "aa").len(), 2);
    }

    #[test]
    fn empty_query_matches_nothing() {
        let buffer = Buffer::from_text("anything", None);
        assert!(find_matches(&buffer, "").is_empty());
    }

    #[test]
    fn wide_characters_are_matched_by_character_not_by_cell() {
        let buffer = Buffer::from_text("한글abc", None);
        let matches = find_matches(&buffer, "글a");
        assert_eq!(matches.len(), 1);
        assert_eq!(buffer.position_of(matches[0].start), (0, 2));
    }
}
