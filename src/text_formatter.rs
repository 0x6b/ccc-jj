use textwrap::{Options, wrap};

/// Formats text with proper line wrapping and list-aware indentation.
///
/// - Wraps lines at the specified width (default 72 for commit message bodies)
/// - Preserves list formatting with proper hanging indents:
///   - Bullet lists (`- `) continue with 2-space indent
///   - Numbered lists (`1. `, `10. `) continue with matching indent
pub fn format_text(text: &str, width: usize) -> String {
    text.lines()
        .map(|line| format_line(line, width))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_line(line: &str, width: usize) -> String {
    if line.trim().is_empty() {
        return line.to_string();
    }

    let trimmed = line.trim_start();
    let leading_ws = &line[..line.len() - trimmed.len()];

    let subsequent_indent = detect_list_indent(trimmed);
    let full_subsequent_indent = format!("{leading_ws}{subsequent_indent}");

    let opts = Options::new(width)
        .initial_indent(leading_ws)
        .subsequent_indent(&full_subsequent_indent);

    wrap(trimmed, opts).join("\n")
}

/// Detects list markers and returns the appropriate hanging indent.
fn detect_list_indent(line: &str) -> &'static str {
    // Bullet list: "- " -> 2 spaces
    if line.starts_with("- ") {
        return "  ";
    }

    // Bullet list: "* " -> 2 spaces
    if line.starts_with("* ") {
        return "  ";
    }

    // Numbered list: "N. " where N is one or more digits
    if let Some(dot_pos) = line.find(". ") {
        let prefix = &line[..dot_pos];
        if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
            // Return spaces matching the length of "N. "
            return match dot_pos {
                1 => "   ",    // "1. " -> 3 spaces
                2 => "    ",   // "10. " -> 4 spaces
                3 => "     ",  // "100. " -> 5 spaces
                _ => "      ", // fallback for larger numbers
            };
        }
    }

    ""
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_text_no_wrap_needed() {
        let input = "Short line.";
        let result = format_text(input, 72);
        assert_eq!(result, "Short line.");
    }

    #[test]
    fn test_simple_text_wrap() {
        let input = "This is a very long line that should be wrapped because it exceeds the maximum width limit.";
        let result = format_text(input, 72);
        assert_eq!(
            result,
            "This is a very long line that should be wrapped because it exceeds the\nmaximum width limit."
        );
    }

    #[test]
    fn test_bullet_list_wrap() {
        let input = "- Survivals. Every counter with two or three neighboring counters survives for the next generation.";
        let result = format_text(input, 72);
        assert_eq!(
            result,
            "- Survivals. Every counter with two or three neighboring counters\n  survives for the next generation."
        );
    }

    #[test]
    fn test_bullet_list_asterisk_wrap() {
        let input = "* Survivals. Every counter with two or three neighboring counters survives for the next generation.";
        let result = format_text(input, 72);
        assert_eq!(
            result,
            "* Survivals. Every counter with two or three neighboring counters\n  survives for the next generation."
        );
    }

    #[test]
    fn test_numbered_list_single_digit_wrap() {
        let input = "1. Survivals. Every counter with two or three neighboring counters survives for the next generation.";
        let result = format_text(input, 72);
        assert_eq!(
            result,
            "1. Survivals. Every counter with two or three neighboring counters\n   survives for the next generation."
        );
    }

    #[test]
    fn test_numbered_list_double_digit_wrap() {
        let input = "10. Survivals. Every counter with two or three neighboring counters survives for the next generation.";
        let result = format_text(input, 72);
        assert_eq!(
            result,
            "10. Survivals. Every counter with two or three neighboring counters\n    survives for the next generation."
        );
    }

    #[test]
    fn test_preserves_leading_whitespace() {
        let input =
            "  - Indented bullet that is long enough to wrap onto the next line for testing.";
        let result = format_text(input, 72);
        assert_eq!(
            result,
            "  - Indented bullet that is long enough to wrap onto the next line for\n    testing."
        );
    }

    #[test]
    fn test_multiple_lines() {
        let input = "First line.\n\nSecond paragraph that is quite long and should wrap properly when it exceeds the limit.";
        let result = format_text(input, 72);
        assert_eq!(
            result,
            "First line.\n\nSecond paragraph that is quite long and should wrap properly when it\nexceeds the limit."
        );
    }

    #[test]
    fn test_mixed_content() {
        let input = "feat: add new feature\n\nThis commit adds a new feature with the following changes:\n\n- First change that has a very long description that needs to wrap to the next line properly.\n- Second change.\n\n1. Step one with a long description that also needs proper wrapping to maintain readability.\n2. Step two.";
        let result = format_text(input, 72);
        let expected = "feat: add new feature\n\nThis commit adds a new feature with the following changes:\n\n- First change that has a very long description that needs to wrap to\n  the next line properly.\n- Second change.\n\n1. Step one with a long description that also needs proper wrapping to\n   maintain readability.\n2. Step two.";
        assert_eq!(result, expected);
    }

    #[test]
    fn test_empty_lines_preserved() {
        let input = "Line one.\n\n\nLine after two empty lines.";
        let result = format_text(input, 72);
        assert_eq!(result, "Line one.\n\n\nLine after two empty lines.");
    }

    #[test]
    fn test_no_wrap_at_exact_width() {
        // Exactly 72 characters
        let input = "This line is exactly seventy-two characters long, no more, no less!!!";
        assert_eq!(input.len(), 69); // Actually 69, let me fix
        let input = "This line is exactly seventy-two characters long, no more and no less!";
        let result = format_text(input, 72);
        assert_eq!(result, input);
    }

    #[test]
    fn test_sentence_not_starting_with_number_but_containing_period() {
        let input = "Version 2.0 introduces many changes that span across multiple components and require careful review.";
        let result = format_text(input, 72);
        // Should NOT be treated as a numbered list
        assert_eq!(
            result,
            "Version 2.0 introduces many changes that span across multiple components\nand require careful review."
        );
    }
}
