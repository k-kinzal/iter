//! Spell-check / suggestion helpers and shared keyword parsers.

use crate::ast::PriorityKeyword;

pub(super) fn parse_priority(name: &str) -> Option<PriorityKeyword> {
    Some(match name {
        "low" => PriorityKeyword::Low,
        "normal" => PriorityKeyword::Normal,
        "high" => PriorityKeyword::High,
        "critical" => PriorityKeyword::Critical,
        _ => return None,
    })
}

pub(super) fn closest<'a>(needle: &str, options: &'a [&'a str]) -> Option<&'a str> {
    let mut best: Option<(&str, usize)> = None;
    for opt in options {
        let d = levenshtein(needle, opt);
        if d <= needle.len().max(opt.len()) / 2 + 1 {
            match best {
                Some((_, bd)) if bd <= d => {}
                _ => best = Some((opt, d)),
            }
        }
    }
    best.map(|(s, _)| s)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut dp = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in dp.iter_mut().enumerate().take(a.len() + 1) {
        row[0] = i;
    }
    for j in 0..=b.len() {
        dp[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }
    dp[a.len()][b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", "abc"), 0);
    }
}
