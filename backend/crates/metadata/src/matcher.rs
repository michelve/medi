//! Best-match scoring shared by the providers and the enrichment orchestration.
//!
//! Given a parsed `(title, year)` from a filename and a set of provider candidates, we
//! must pick the right one — or decide none is good enough and mark the title
//! `unmatched` (`docs/.tasks/60` §Sub-tasks 4). The score combines a normalized
//! title-similarity with a year agreement bonus so an exact-year, near-exact-title hit
//! beats a same-title, wrong-year remake.

/// A candidate cleared for auto-match must reach this combined score. Below it, the
/// title is marked `unmatched` and left for a manual `POST /api/movies/:id/match`.
pub const MATCH_THRESHOLD: f64 = 0.6;

/// Normalize a title for comparison: lowercase, strip non-alphanumerics to single
/// spaces, drop a leading English article, collapse whitespace. Mirrors the spirit of
/// `scanner::sort_title` but keeps interior words (we compare full titles, not sort
/// keys).
pub fn normalize(title: &str) -> String {
    let lowered = title.to_lowercase();
    let mut cleaned = String::with_capacity(lowered.len());
    let mut prev_space = false;
    for ch in lowered.chars() {
        if ch.is_alphanumeric() {
            cleaned.push(ch);
            prev_space = false;
        } else if !prev_space {
            cleaned.push(' ');
            prev_space = true;
        }
    }
    let trimmed = cleaned.trim();
    for article in ["the ", "a ", "an "] {
        if let Some(rest) = trimmed.strip_prefix(article) {
            return rest.to_string();
        }
    }
    trimmed.to_string()
}

/// Similarity of two titles in `[0.0, 1.0]`: the Sørensen–Dice coefficient over word
/// bigrams of the normalized strings, with an exact-equality fast path. Dice handles
/// word reorderings and small edits gracefully and needs no external crate.
pub fn title_similarity(a: &str, b: &str) -> f64 {
    let na = normalize(a);
    let nb = normalize(b);
    if na == nb {
        return 1.0;
    }
    if na.is_empty() || nb.is_empty() {
        return 0.0;
    }
    let ga = bigrams(&na);
    let gb = bigrams(&nb);
    if ga.is_empty() || gb.is_empty() {
        // One-character strings have no bigrams: fall back to equality (handled above)
        // → here they differ, so 0.
        return 0.0;
    }
    let mut shared = 0usize;
    let mut gb_used = vec![false; gb.len()];
    for x in &ga {
        for (i, y) in gb.iter().enumerate() {
            if !gb_used[i] && x == y {
                gb_used[i] = true;
                shared += 1;
                break;
            }
        }
    }
    (2.0 * shared as f64) / (ga.len() + gb.len()) as f64
}

/// Adjacent character bigrams of a string (spaces included — they mark word joins).
fn bigrams(s: &str) -> Vec<[char; 2]> {
    let chars: Vec<char> = s.chars().collect();
    chars.windows(2).map(|w| [w[0], w[1]]).collect()
}

/// The combined match score for a candidate against the parsed `(title, year)`.
///
/// Base is the title similarity. A year match is a strong signal: an exact-year hit
/// adds a bonus, a ±1 year (release-vs-listing drift) a smaller one, and a *conflicting*
/// year applies a penalty so a same-title wrong-year remake does not out-score the real
/// one. When the parsed title has no year we cannot penalize, so the score is the title
/// similarity alone.
pub fn score(parsed_title: &str, parsed_year: Option<i64>, cand_title: &str, cand_year: Option<i64>) -> f64 {
    let sim = title_similarity(parsed_title, cand_title);
    let year_adj = match (parsed_year, cand_year) {
        (Some(py), Some(cy)) => {
            let d = (py - cy).abs();
            if d == 0 {
                0.15
            } else if d == 1 {
                0.05
            } else {
                -0.25
            }
        }
        // No year to compare on one side ⇒ no adjustment.
        _ => 0.0,
    };
    (sim + year_adj).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_articles_and_punctuation() {
        assert_eq!(normalize("The Matrix"), "matrix");
        assert_eq!(normalize("Blade Runner 2049"), "blade runner 2049");
        assert_eq!(normalize("WALL·E"), "wall e");
        assert_eq!(normalize("Amélie"), "amélie");
    }

    #[test]
    fn identical_titles_score_one() {
        assert_eq!(title_similarity("Arrival", "arrival"), 1.0);
        assert_eq!(title_similarity("The Matrix", "Matrix"), 1.0);
    }

    #[test]
    fn near_titles_score_high_distant_low() {
        assert!(title_similarity("Blade Runner 2049", "Blade Runner: 2049") > 0.9);
        assert!(title_similarity("Arrival", "Departures") < 0.5);
    }

    #[test]
    fn exact_year_beats_wrong_year_remake() {
        // Two "Arrival" candidates; the 2016 one wins over a hypothetical 1996 remake.
        let good = score("Arrival", Some(2016), "Arrival", Some(2016));
        let remake = score("Arrival", Some(2016), "Arrival", Some(1996));
        assert!(good > remake);
        assert!(good >= MATCH_THRESHOLD);
    }

    #[test]
    fn below_threshold_titles_do_not_match() {
        let s = score("Arrival", Some(2016), "The Departed", Some(2006));
        assert!(s < MATCH_THRESHOLD, "unrelated title stays under threshold: {s}");
    }
}
