//! Small formatting helpers for the chrome (`format.ts`): counts, sizes,
//! ages, engine names, connection tile hues, and the fuzzy matcher of the
//! ⌘K palette.

use super::js::{is_js_space, js_round, to_fixed};

/// 1234 → "1.2k", 98120004 → "98M".
pub fn compact_count(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    for (v, u) in [(1e9, "B"), (1e6, "M"), (1e3, "k")] {
        let x = n as f64;
        if x >= v {
            let x = x / v;
            let s = if x < 10.0 {
                let f = to_fixed(x, 1);
                f.strip_suffix(".0").map(str::to_owned).unwrap_or(f)
            } else {
                format!("{}", js_round(x))
            };
            return format!("{s}{u}");
        }
    }
    n.to_string()
}

/// A byte size: "512 B", "1.5 KB", "12 MB".
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut i = 0;
    let mut x = n as f64;
    while x >= 1024.0 && i < UNITS.len() - 1 {
        x /= 1024.0;
        i += 1;
    }
    let num = if x < 10.0 && i > 0 {
        to_fixed(x, 1)
    } else {
        format!("{}", js_round(x))
    };
    format!("{num} {}", UNITS[i])
}

/// A running clock: 400 → "0.4 s", 12340 → "12.3 s", 75000 → "1 m 15 s".
pub fn elapsed(ms: i64) -> String {
    let t = ms.max(0);
    if t < 60_000 {
        let tenths = t / 100;
        return format!("{}.{} s", tenths / 10, tenths % 10);
    }
    let s = t / 1000;
    format!("{} m {:02} s", s / 60, s % 60)
}

/// How long ago `ms` (epoch milliseconds) was, at `now`: "now", "5m", "3h", "2d".
pub fn ago(ms: i64, now: i64) -> String {
    let s = js_round((now - ms) as f64 / 1000.0).max(0.0);
    if s < 45.0 {
        "now".into()
    } else if s < 3600.0 {
        format!("{}m", js_round(s / 60.0))
    } else if s < 86400.0 {
        format!("{}h", js_round(s / 3600.0))
    } else {
        format!("{}d", js_round(s / 86400.0))
    }
}

/// A database engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Engine {
    Postgres,
    Mysql,
    Sqlite,
}

impl Engine {
    /// Display name: "PostgreSQL", "MySQL", "SQLite".
    pub fn label(self) -> &'static str {
        match self {
            Self::Postgres => "PostgreSQL",
            Self::Mysql => "MySQL",
            Self::Sqlite => "SQLite",
        }
    }
}

/// Stable hue (0–359) per name, for the coloured connection tiles (the UI
/// draws `oklch(var(--tile-l) var(--tile-c) <hue>)`).
pub fn tile_hue(name: &str) -> u32 {
    let mut h: u32 = 0;
    for c in name.chars() {
        // `charCodeAt(0)` of each code point: its first UTF-16 unit.
        let mut buf = [0u16; 2];
        let unit = c.encode_utf16(&mut buf)[0];
        h = h.wrapping_mul(31).wrapping_add(u32::from(unit));
    }
    h % 360
}

// ── Fuzzy matching (⌘K palette) ───────────────────────────────────────────

/// A fuzzy match: its score (higher is better) and the matched character
/// indices (`char` positions in the text).
#[derive(Debug, Clone, PartialEq)]
pub struct FuzzyHit {
    pub score: f64,
    pub idxs: Vec<usize>,
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Subsequence match; consecutive and word-start characters score higher.
pub fn fuzzy(q: &str, text: &str) -> Option<FuzzyHit> {
    let t: Vec<char> = text.chars().map(lower).collect();
    let mut i = 0;
    let mut score = 0.0;
    let mut prev: isize = -2;
    let mut idxs = Vec::new();
    for ch in q.to_lowercase().chars() {
        let j = i + t.get(i..)?.iter().position(|&c| c == ch)?;
        idxs.push(j);
        score += if j as isize == prev + 1 { 3.0 } else { 1.0 };
        if j == 0 || matches!(t[j - 1], '.' | '_' | '-' | '/') || is_js_space(t[j - 1]) {
            score += 2.0;
        }
        prev = j as isize;
        i = j + 1;
    }
    Some(FuzzyHit {
        score: score - t.len() as f64 * 0.01,
        idxs,
    })
}

/// A run of text, highlighted or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightRun {
    pub text: String,
    pub hit: bool,
}

/// Splits `text` into runs for highlighting the `char` indices `idxs`.
pub fn highlight_runs(text: &str, idxs: &[usize]) -> Vec<HighlightRun> {
    let mut out: Vec<HighlightRun> = Vec::new();
    for (i, c) in text.chars().enumerate() {
        let hit = idxs.contains(&i);
        match out.last_mut() {
            Some(last) if last.hit == hit => last.text.push(c),
            _ => out.push(HighlightRun {
                text: c.to_string(),
                hit,
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_counts_tenths_of_a_second_under_a_minute() {
        assert_eq!(elapsed(0), "0.0 s");
        assert_eq!(elapsed(449), "0.4 s");
        assert_eq!(elapsed(12_340), "12.3 s");
        assert_eq!(elapsed(59_999), "59.9 s");
    }

    #[test]
    fn elapsed_switches_to_minutes_and_seconds() {
        assert_eq!(elapsed(60_000), "1 m 00 s");
        assert_eq!(elapsed(75_900), "1 m 15 s");
    }

    #[test]
    fn elapsed_never_goes_negative() {
        assert_eq!(elapsed(-50), "0.0 s");
    }

    // Not in format.test.ts: the doc comments' examples.
    #[test]
    fn compact_count_and_bytes() {
        assert_eq!(compact_count(999), "999");
        assert_eq!(compact_count(1234), "1.2k");
        assert_eq!(compact_count(1000), "1k");
        assert_eq!(compact_count(1250), "1.3k");
        assert_eq!(compact_count(98_120_004), "98M");
        assert_eq!(compact_count(2_500_000_000), "2.5B");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(12 * 1024 * 1024), "12 MB");
    }

    #[test]
    fn ago_rounds_to_the_unit() {
        let now = 1_000_000_000;
        assert_eq!(ago(now - 10_000, now), "now");
        assert_eq!(ago(now - 5 * 60_000, now), "5m");
        assert_eq!(ago(now - 3 * 3_600_000, now), "3h");
        assert_eq!(ago(now - 2 * 86_400_000, now), "2d");
        assert_eq!(ago(now + 10_000, now), "now");
    }

    #[test]
    fn fuzzy_matches_subsequences() {
        assert!(fuzzy("xyz", "orders").is_none());
        let hit = fuzzy("ord", "public.orders").unwrap();
        assert_eq!(hit.idxs, vec![7, 8, 9]);
        let spread = fuzzy("ord", "of record").unwrap();
        assert!(hit.score > spread.score);
        assert_eq!(
            highlight_runs("abc", &[1]),
            vec![
                HighlightRun {
                    text: "a".into(),
                    hit: false
                },
                HighlightRun {
                    text: "b".into(),
                    hit: true
                },
                HighlightRun {
                    text: "c".into(),
                    hit: false
                },
            ]
        );
        assert!(tile_hue("prod") < 360);
        assert_eq!(tile_hue("prod"), tile_hue("prod"));
    }
}
