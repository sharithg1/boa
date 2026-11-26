//! Small helpers that reproduce JavaScript semantics the ported code relied
//! on (`String.prototype.trim`, `\s`, `\w`, `Math.round`, `toFixed`,
//! `JSON.stringify` of a string).

/// JavaScript's `\s`: Unicode white space plus the BOM.
pub(crate) fn is_js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// `String.prototype.trim`.
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// JavaScript's (ASCII) `\w`.
pub(crate) fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `Math.round`: halves round towards +∞.
pub(crate) fn js_round(x: f64) -> f64 {
    let f = x.floor();
    if x - f >= 0.5 { f + 1.0 } else { f }
}

/// `Number.prototype.toFixed`: rounds the exact value half up.
pub(crate) fn to_fixed(x: f64, digits: usize) -> String {
    // Near-exact decimal expansion, then round half up by hand (Rust's
    // formatting rounds exact ties to even, toFixed rounds them up).
    let s = format!("{:.*}", digits + 30, x.abs());
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let mut ds: Vec<u8> = int.bytes().chain(frac.bytes().take(digits)).collect();
    if frac.as_bytes().get(digits).is_some_and(|&d| d >= b'5') {
        let mut k = ds.len();
        loop {
            if k == 0 {
                ds.insert(0, b'1');
                break;
            }
            k -= 1;
            if ds[k] == b'9' {
                ds[k] = b'0';
            } else {
                ds[k] += 1;
                break;
            }
        }
    }
    let split = ds.len() - digits;
    let mut out = String::new();
    if x < 0.0 {
        out.push('-');
    }
    out.push_str(std::str::from_utf8(&ds[..split]).unwrap_or("0"));
    if digits > 0 {
        out.push('.');
        out.push_str(std::str::from_utf8(&ds[split..]).unwrap_or(""));
    }
    out
}

/// `JSON.stringify` of a string: quoted, with `"`, `\` and control
/// characters escaped.
pub(crate) fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str("\\u");
                out.push_str(&format!("{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_fixed_rounds_half_up() {
        assert_eq!(to_fixed(1.25, 1), "1.3");
        assert_eq!(to_fixed(0.05, 1), "0.1");
        assert_eq!(to_fixed(9.96, 1), "10.0");
        assert_eq!(to_fixed(1.0, 1), "1.0");
        assert_eq!(to_fixed(12.3, 0), "12");
    }

    #[test]
    fn js_round_ties_up() {
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(2.4), 2.0);
    }

    #[test]
    fn json_string_escapes() {
        let control = char::from(1u8).to_string();
        let expected = ["\"a\\\"b\\\\c\\nd\\", "u0001\""].concat();
        assert_eq!(json_string(&format!("a\"b\\c\nd{control}")), expected);
    }
}
