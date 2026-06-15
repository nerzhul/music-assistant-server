//! `create_safe_string` equivalent — a Rust port of the Python
//! helper used for case-insensitive search on the library.
//!
//! V1: ASCII-only lowercasing + diacritic-stripping fallback
//! (non-ASCII characters are mapped to their closest ASCII
//! approximation when possible; otherwise dropped). This isn't a
//! full `unidecode` port — western album titles (most of the MA
//! catalogue) round-trip correctly, but e.g. Cyrillic / CJK
//! characters fall through and are stripped. A future PR can swap
//! in the `deunicode` crate (≈ 1 MiB binary) without changing the
//! public API.

/// Lowercase + ASCII-fold + collapse non-alphanumeric (keep spaces).
pub fn create_safe_string(input: &str) -> String {
    let lower = input.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if c == ' ' || c == '\t' {
            if !out.ends_with(' ') {
                out.push(' ');
            }
        } else if !c.is_ascii() {
            // Best-effort fold for common Western diacritics.
            // If `deunicode` ever lands, this is where we'd call it.
            for fc in fold_non_ascii(c).chars() {
                if fc.is_ascii_alphanumeric() {
                    out.push(fc.to_ascii_lowercase());
                }
            }
        }
    }
    out.trim().to_string()
}

fn fold_non_ascii(c: char) -> &'static str {
    // Pre-composed Latin-1 Supplement + Latin Extended-A covers
    // ~all Western album titles.
    match c {
        // Lowercase variants that `to_lowercase` already handled
        // — but include them in case `to_lowercase` was a no-op
        // (e.g. Turkish dotless i).
        '\u{00E0}' | '\u{00E1}' | '\u{00E2}' | '\u{00E3}' | '\u{00E4}' | '\u{00E5}' => "a",
        '\u{00E6}' => "ae", // æ ligature
        '\u{00C6}' => "ae", // Æ ligature
        '\u{00E7}' => "c",
        '\u{00E8}' | '\u{00E9}' | '\u{00EA}' | '\u{00EB}' => "e",
        '\u{00EC}' | '\u{00ED}' | '\u{00EE}' | '\u{00EF}' => "i",
        '\u{00F1}' => "n",
        '\u{00F2}' | '\u{00F3}' | '\u{00F4}' | '\u{00F5}' | '\u{00F6}' | '\u{00F8}' => "o",
        '\u{00DF}' => "ss", // ß
        '\u{00F9}' | '\u{00FA}' | '\u{00FB}' | '\u{00FC}' => "u",
        '\u{00FD}' | '\u{00FF}' => "y",
        // Latin Extended-A (a subset, the common ones)
        '\u{0101}' | '\u{0103}' | '\u{0105}' => "a",
        '\u{0107}' | '\u{010D}' => "c",
        '\u{010F}' | '\u{0111}' => "d",
        '\u{0113}' | '\u{0115}' | '\u{0117}' | '\u{0119}' | '\u{011B}' => "e",
        '\u{011F}' | '\u{0121}' | '\u{0123}' | '\u{0125}' | '\u{0127}' => "g",
        '\u{0129}' | '\u{012B}' | '\u{012D}' | '\u{012F}' => "i",
        '\u{0131}' => "i", // dotless i
        '\u{0133}' => "n", // nj → n
        '\u{0135}' => "o", // oj → o
        '\u{0137}' | '\u{0138}' => "k",
        '\u{013A}' | '\u{013C}' | '\u{013E}' | '\u{0140}' | '\u{0142}' => "l",
        '\u{0144}' | '\u{0146}' | '\u{0148}' => "n",
        '\u{014D}' | '\u{014F}' | '\u{0151}' => "o",
        '\u{0153}' | '\u{0152}' => "oe", // œ / Œ ligature
        '\u{0155}' | '\u{0157}' | '\u{0159}' | '\u{015B}' | '\u{015D}' | '\u{015F}' => "r",
        '\u{0161}' | '\u{0163}' | '\u{0165}' | '\u{0167}' => "s",
        '\u{0169}' | '\u{016B}' | '\u{016D}' | '\u{016F}' | '\u{0171}' | '\u{0173}' => "u",
        '\u{0175}' | '\u{0177}' => "y",
        '\u{017A}' | '\u{017C}' | '\u{017E}' => "z",
        // Punctuation that we keep as space-equivalent
        // (handled by the main loop)
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_passes_through() {
        assert_eq!(create_safe_string("Hello World"), "hello world");
    }

    #[test]
    fn strips_punctuation() {
        assert_eq!(
            create_safe_string("Hello, World! (2024)"),
            "hello world 2024"
        );
    }

    #[test]
    fn collapses_whitespace() {
        assert_eq!(create_safe_string("a   b\tc"), "a b c");
    }

    #[test]
    fn folds_western_diacritics() {
        assert_eq!(create_safe_string("Café Olé"), "cafe ole");
        assert_eq!(create_safe_string("naïve"), "naive");
        assert_eq!(create_safe_string("Björk"), "bjork");
        assert_eq!(create_safe_string("Æon"), "aeon");
        assert_eq!(create_safe_string("Über"), "uber");
        assert_eq!(create_safe_string("Señor"), "senor");
        assert_eq!(create_safe_string("Voilà"), "voila");
        assert_eq!(create_safe_string("Straße"), "strasse");
        assert_eq!(create_safe_string("Œdipus"), "oedipus");
    }

    #[test]
    fn empty_and_whitespace() {
        assert_eq!(create_safe_string(""), "");
        assert_eq!(create_safe_string("   "), "");
    }

    #[test]
    fn non_western_dropped() {
        // Cyrillic / CJK fall through to None; in V1 the chars are
        // dropped (replaced by a single space between alphanumeric
        // runs).
        assert_eq!(create_safe_string("AC/DC"), "acdc");
        // Cyrillic alone: only the ASCII letters survive.
        assert_eq!(create_safe_string("Мир"), "");
    }
}
