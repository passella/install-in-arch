use std::fmt::Write as _;

pub fn is_bidi_or_invisible_format(ch: char) -> bool {
    matches!(
        ch,
        '\u{061C}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

pub fn sanitize_single_line_value(raw: &str) -> String {
    let mut clean = String::with_capacity(raw.len());
    let mut last_space = false;
    for ch in raw.chars() {
        if is_bidi_or_invisible_format(ch) {
            continue;
        }
        if ch.is_control() || ch.is_whitespace() {
            if !last_space && !clean.is_empty() {
                clean.push(' ');
                last_space = true;
            }
        } else {
            clean.push(ch);
            last_space = false;
        }
    }
    clean.trim().to_string()
}

pub fn sanitize_terminal_text(raw: &str) -> String {
    let stripped = strip_ansi_sequences(raw);
    sanitize_single_line_value(&stripped)
}

pub fn sanitize_multiline_script_text(raw: &str) -> String {
    let stripped = strip_ansi_sequences(raw);
    let mut out = String::with_capacity(stripped.len());
    for ch in stripped.chars() {
        if ch == '\r' {
            continue;
        }
        if ch == '\n' || ch == '\t' {
            out.push(ch);
        } else if ch.is_control() {
            let _ = write!(out, "{}", ch.escape_default());
        } else if is_bidi_or_invisible_format(ch) {
            let _ = write!(out, "{}", ch.escape_unicode());
        } else {
            out.push(ch);
        }
    }
    out
}

pub fn sanitize_for_error(raw: &str) -> String {
    const MAX_ERROR_PREVIEW_CHARS: usize = 512;
    let mut out = String::with_capacity(raw.len().min(MAX_ERROR_PREVIEW_CHARS));
    for (count, ch) in raw.chars().enumerate() {
        if count >= MAX_ERROR_PREVIEW_CHARS {
            out.push_str("...");
            break;
        }
        if ch.is_control() {
            let _ = write!(out, "{}", ch.escape_default());
        } else if is_bidi_or_invisible_format(ch) {
            let _ = write!(out, "{}", ch.escape_unicode());
        } else {
            out.push(ch);
        }
    }
    out
}

fn strip_ansi_sequences(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    for next_ch in chars.by_ref() {
                        if ('@'..='~').contains(&next_ch) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(next_ch) = chars.next() {
                        if next_ch == '\x07' || next_ch == '\u{009c}' {
                            break;
                        }
                        if next_ch == '\x1b' && chars.peek().copied() == Some('\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(next_ch) if ('\x20'..='\x7e').contains(&next_ch) => {
                    chars.next();
                }
                _ => {}
            }
            continue;
        }

        if ch == '\u{009b}' {
            for next_ch in chars.by_ref() {
                if ('@'..='~').contains(&next_ch) {
                    break;
                }
            }
            continue;
        }

        if ch == '\u{009d}' {
            while let Some(next_ch) = chars.next() {
                if next_ch == '\x07' || next_ch == '\u{009c}' {
                    break;
                }
                if next_ch == '\x1b' && chars.peek().copied() == Some('\\') {
                    chars.next();
                    break;
                }
            }
            continue;
        }

        out.push(ch);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::{
        is_bidi_or_invisible_format, sanitize_for_error, sanitize_multiline_script_text,
        sanitize_single_line_value, sanitize_terminal_text,
    };

    #[test]
    fn sanitizes_multiline_script_preserving_newlines_and_tabs_while_stripping_ansi() {
        let raw = "#!/bin/sh\r\n\techo \x1b[31mhi\x1b[0m\x08\u{202E}there\nexit 0\n";
        let clean = sanitize_multiline_script_text(raw);
        assert_eq!(clean, "#!/bin/sh\n\techo hi\\u{8}\\u{202e}there\nexit 0\n");
    }

    #[test]
    fn strips_ansi_csi_osc_and_bidi_controls_in_terminal_text() {
        let input = "\x1b[31mEvil\x1b[0m\x1b]2;pwned\x07 \u{202E}txt\r\nnext\u{009b}2J";
        let clean = sanitize_terminal_text(input);
        assert_eq!(clean, "Evil txt next");
        assert!(!clean.chars().any(char::is_control));
        assert!(!clean.chars().any(is_bidi_or_invisible_format));

        let single = sanitize_single_line_value("hello\u{202E}\u{2066}\nworld\u{FEFF}");
        assert_eq!(single, "hello world");
    }

    #[test]
    fn escapes_control_and_bidi_characters_in_error_strings() {
        let input = "bad\x1b[31m\r\n\0\u{202E}name";
        let escaped = sanitize_for_error(input);
        assert_eq!(escaped, "bad\\u{1b}[31m\\r\\n\\u{0}\\u{202e}name");
        assert!(!escaped.chars().any(char::is_control));
        assert!(!escaped.chars().any(is_bidi_or_invisible_format));

        let long = "a".repeat(600);
        let truncated = sanitize_for_error(&long);
        assert!(truncated.ends_with("..."));
        assert!(truncated.len() < 520);
    }
}
