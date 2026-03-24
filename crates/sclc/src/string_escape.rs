//! SCL string escape handling.
//!
//! SCL strings support the following escape sequences:
//!
//! | Escape   | Character       |
//! |----------|-----------------|
//! | `\n`     | Line feed       |
//! | `\r`     | Carriage return |
//! | `\t`     | Tab             |
//! | `\\`     | Backslash       |
//! | `\{`     | Literal `{`     |
//! | `\"`     | Double quote    |
//!
//! Inside string literals, an unescaped `{` begins an interpolation
//! expression (handled by the lexer/parser, not here).
//!
//! Unrecognised escape sequences are kept verbatim (the backslash and
//! the following character are both emitted).

/// Decode a raw string literal body (the text between quotes, with
/// interpolation segments already split out) into its runtime value.
pub fn decode_string(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars();

    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }

        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some('{') => out.push('{'),
            Some('"') => out.push('"'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }

    out
}

/// Encode a runtime string value back to its source representation
/// (without surrounding quotes). This is the inverse of [`decode_string`].
pub fn encode_string(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '"' => out.push_str("\\\""),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_basic() {
        let original = "hello\nworld\t{foo}\\bar\"baz";
        let encoded = encode_string(original);
        let decoded = decode_string(&encoded);
        assert_eq!(decoded, original);
    }

    #[test]
    fn decode_unrecognised_escape() {
        // Unrecognised escapes are kept verbatim.
        assert_eq!(decode_string(r"\z"), "\\z");
    }

    #[test]
    fn decode_trailing_backslash() {
        assert_eq!(decode_string("abc\\"), "abc\\");
    }

    #[test]
    fn encode_special_chars() {
        assert_eq!(encode_string("\n"), "\\n");
        assert_eq!(encode_string("\r"), "\\r");
        assert_eq!(encode_string("\t"), "\\t");
        assert_eq!(encode_string("\\"), "\\\\");
        assert_eq!(encode_string("{"), "\\{");
        assert_eq!(encode_string("\""), "\\\"");
    }

    #[test]
    fn decode_all_escapes() {
        assert_eq!(decode_string(r"\n"), "\n");
        assert_eq!(decode_string(r"\r"), "\r");
        assert_eq!(decode_string(r"\t"), "\t");
        assert_eq!(decode_string(r"\\"), "\\");
        assert_eq!(decode_string(r"\{"), "{");
        assert_eq!(decode_string(r#"\""#), "\"");
    }
}
