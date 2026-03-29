//! Unicode display-width truncation for TUI single-line text.

use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

pub(crate) fn truncate_fit(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    const ELLIPSIS: &str = "…";
    let ellipsis_width = UnicodeWidthStr::width(ELLIPSIS);
    if max_width <= ellipsis_width {
        return ELLIPSIS.chars().take(max_width).collect();
    }
    let budget = max_width - ellipsis_width;
    let mut acc = 0usize;
    let mut end_byte = 0;
    for (i, ch) in text.char_indices() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if acc + cw > budget {
            break;
        }
        acc += cw;
        end_byte = i + ch.len_utf8();
    }
    format!("{}{}", &text[..end_byte], ELLIPSIS)
}

/// Truncate with middle ellipsis when wider than `max_width` (display width).
pub(crate) fn truncate_middle_fit(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    const ELLIPSIS: &str = "…";
    let ew = UnicodeWidthStr::width(ELLIPSIS);
    if max_width <= ew {
        return ELLIPSIS.chars().take(max_width).collect();
    }
    let inner_budget = max_width - ew;
    if inner_budget <= 1 {
        return truncate_fit(text, max_width);
    }
    let take_each = inner_budget / 2;
    let prefix = truncate_prefix_fit(text, take_each);
    let suffix_budget = inner_budget.saturating_sub(UnicodeWidthStr::width(prefix.as_str()));
    if suffix_budget == 0 {
        return format!("{prefix}{ELLIPSIS}");
    }
    let mut suffix = String::new();
    let mut acc = 0usize;
    for ch in text.chars().rev() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if acc + cw > suffix_budget {
            break;
        }
        suffix.push(ch);
        acc += cw;
    }
    let suffix: String = suffix.chars().rev().collect();
    format!("{prefix}{ELLIPSIS}{suffix}")
}

pub(crate) fn truncate_prefix_fit(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    let mut acc = 0usize;
    let mut end_byte = 0;
    for (i, ch) in text.char_indices() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if acc + cw > max_width {
            break;
        }
        acc += cw;
        end_byte = i + ch.len_utf8();
    }
    text[..end_byte].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_fit_shortens() {
        let s = truncate_fit("hello world", 5);
        assert!(s.contains('…'));
        assert!(UnicodeWidthStr::width(s.as_str()) <= 5);
    }

    #[test]
    fn truncate_prefix_fits_display_width() {
        let s = truncate_prefix_fit("hello world", 5);
        assert!(UnicodeWidthStr::width(s.as_str()) <= 5);
        assert!(s.starts_with("hello"));
    }

    #[test]
    fn truncate_middle_fits_display_width() {
        let s = truncate_middle_fit("crates/quorp_tui/src/chat.rs", 16);
        assert!(UnicodeWidthStr::width(s.as_str()) <= 16);
        assert!(s.contains('…'));
    }
}
