//! Character classes shared by the lexers, matching the JS regexes of the
//! TypeScript original (see the module docs of `sql`).

/// `[A-Za-z_À-￿]`: starts a word.
pub(crate) fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c as u32 >= 0xC0
}

/// `[\w$À-￿]`: continues a word.
pub(crate) fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$' || c as u32 >= 0xC0
}

/// JS `\s`.
pub(crate) fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{B}' | '\u{C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// JS `s.trim() === ''`.
pub(crate) fn is_blank(s: &str) -> bool {
    s.chars().all(is_space)
}

/// JS `s.trim()`.
#[cfg(test)]
pub(crate) fn trim(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// The char starting at byte `i`.
pub(crate) fn char_at(s: &str, i: usize) -> Option<char> {
    s.get(i..)?.chars().next()
}

/// The char ending at byte `i`.
pub(crate) fn char_before(s: &str, i: usize) -> Option<char> {
    s.get(..i)?.chars().next_back()
}

/// Byte index after the blanks (JS `\s`) starting at `i`.
pub(crate) fn skip_space(s: &str, mut i: usize) -> usize {
    while let Some(c) = char_at(s, i) {
        if !is_space(c) {
            break;
        }
        i += c.len_utf8();
    }
    i
}

/// `i`, clamped to `s` and moved back to a char boundary.
pub(crate) fn floor_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Calls `f` with `word` lower-cased: on the stack for short ASCII words,
/// so keyword lookups on every keystroke do not allocate.
pub(crate) fn with_lower<R>(word: &str, f: impl FnOnce(&str) -> R) -> R {
    const MAX: usize = 32;
    let b = word.as_bytes();
    if b.len() <= MAX && word.is_ascii() {
        let mut buf = [0u8; MAX];
        for (d, s) in buf.iter_mut().zip(b) {
            *d = s.to_ascii_lowercase();
        }
        // ASCII in, ASCII out: always valid UTF-8.
        f(std::str::from_utf8(&buf[..b.len()]).unwrap_or_default())
    } else {
        f(&word.to_lowercase())
    }
}
