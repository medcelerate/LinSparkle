//! Version comparison, following Sparkle's `SUStandardVersionComparator` so
//! an appcast orders the same way on every platform: versions are split into
//! runs of digits, runs of other characters, and `.` separators, compared run
//! by run, numbers numerically.

use std::cmp::Ordering;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Number,
    Text,
    Separator,
}

fn kind_of(c: char) -> Kind {
    if c.is_ascii_digit() {
        Kind::Number
    } else if c == '.' || c.is_whitespace() {
        Kind::Separator
    } else {
        Kind::Text
    }
}

fn parts(version: &str) -> Vec<(Kind, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut current: Option<Kind> = None;
    for (i, c) in version.char_indices() {
        let k = kind_of(c);
        match current {
            // Every separator is its own part; digits and text run on.
            Some(prev) if prev == k && k != Kind::Separator => {}
            Some(prev) => {
                out.push((prev, &version[start..i]));
                start = i;
                current = Some(k);
            }
            None => current = Some(k),
        }
    }
    if let Some(k) = current {
        out.push((k, &version[start..]));
    }
    out
}

fn compare_numbers(a: &str, b: &str) -> Ordering {
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

/// Compare two version strings: `Greater` when `a` is newer than `b`.
pub fn compare(a: &str, b: &str) -> Ordering {
    let pa = parts(a);
    let pb = parts(b);
    for (&(ka, sa), &(kb, sb)) in pa.iter().zip(pb.iter()) {
        let ord = match (ka, kb) {
            (Kind::Number, Kind::Number) => compare_numbers(sa, sb),
            (x, y) if x == y => sa.cmp(sb),
            // Different kinds: text (a pre-release tag) loses to anything,
            // and a number beats a separator.
            (_, Kind::Text) => Ordering::Greater,
            (Kind::Text, _) => Ordering::Less,
            (Kind::Number, _) => Ordering::Greater,
            _ => Ordering::Less,
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    // Equal up to the shorter one. If what the longer one has next is text,
    // it's a pre-release of the shorter (1.0b1 < 1.0); otherwise it's newer
    // (1.0.1 > 1.0).
    match pa.len().cmp(&pb.len()) {
        Ordering::Equal => Ordering::Equal,
        Ordering::Greater => match pa[pb.len()].0 {
            Kind::Text => Ordering::Less,
            _ => Ordering::Greater,
        },
        Ordering::Less => match pb[pa.len()].0 {
            Kind::Text => Ordering::Greater,
            _ => Ordering::Less,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::compare;
    use std::cmp::Ordering::*;

    #[test]
    fn orders_like_sparkle() {
        assert_eq!(compare("1.0", "1.0"), Equal);
        assert_eq!(compare("1.0.1", "1.0"), Greater);
        assert_eq!(compare("1.10", "1.9"), Greater);
        assert_eq!(compare("3.13.4", "3.13.10"), Less);
        assert_eq!(compare("1.0", "1.0b1"), Greater);
        assert_eq!(compare("1.0b2", "1.0b1"), Greater);
        assert_eq!(compare("1.0b1", "1.0a9"), Greater);
        assert_eq!(compare("2.0", "1.99.99"), Greater);
        assert_eq!(compare("1.01", "1.1"), Equal);
        assert_eq!(compare("1.0.0", "1.0"), Greater);
        assert_eq!(compare("4.0.0-beta.1", "4.0.0"), Less);
        assert_eq!(compare("99999999999999999999999", "9"), Greater);
    }
}
