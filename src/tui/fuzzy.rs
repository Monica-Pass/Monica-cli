//! Subsequence scoring for the search field. A term matches when its characters appear in
//! order, so a short abbreviation reaches a long name. Ranking is what makes the tolerance
//! useful: the tighter and better anchored a hit is, the earlier it appears.
//!
//! The alignment is greedy from a chosen start. Besides the plain leftmost alignment, only
//! starts that land on a word boundary are re-scored, because that is the one difference a
//! later start can make that a person notices. A full dynamic-programming alignment would
//! cost more on every keystroke than the few rows it would reorder.

const MATCH: i32 = 16;
/// The term starting at the head of a word. Awarded once per term: scoring it on every
/// character lets a letter-per-word haystack outrank an exact contiguous hit.
const BONUS_ANCHOR: i32 = 16;
const BONUS_CONSECUTIVE: i32 = 12;
const GAP_PENALTY: i32 = 2;

/// Score of `haystack` against a whitespace separated `query`, or `None` when any term is
/// absent. Terms keep their own score so several short words outrank one lucky substring.
pub(super) fn score(haystack: &str, query: &str) -> Option<i32> {
    let haystack: Vec<char> = haystack.to_lowercase().chars().collect();
    let query = query.to_lowercase();
    let mut total = 0;
    for term in query.split_whitespace() {
        let term: Vec<char> = term.chars().collect();
        total += term_score(&haystack, &term)?;
    }
    Some(total)
}

fn term_score(haystack: &[char], term: &[char]) -> Option<i32> {
    if term.is_empty() {
        return Some(0);
    }
    let first = term[0];
    let base = align(haystack, term, haystack.iter().position(|ch| *ch == first)?)?;
    let mut best = base;
    for start in 1..haystack.len() {
        if haystack[start] != first || !is_boundary(haystack, start) {
            continue;
        }
        if let Some(score) = align(haystack, term, start) {
            best = best.max(score);
        }
    }
    Some(best)
}

/// Leftmost alignment of `term` beginning at `start`, which must already hold `term[0]`.
fn align(haystack: &[char], term: &[char], start: usize) -> Option<i32> {
    let anchor = if is_boundary(haystack, start) {
        BONUS_ANCHOR
    } else {
        0
    };
    let mut score = MATCH + anchor;
    if term.len() == 1 {
        return Some(score);
    }
    let mut index = 1;
    let mut previous = start;
    for (position, character) in haystack.iter().enumerate().skip(start + 1) {
        if *character != term[index] {
            continue;
        }
        score += MATCH;
        if position == previous + 1 {
            score += BONUS_CONSECUTIVE;
        }
        previous = position;
        index += 1;
        if index == term.len() {
            let gaps = previous - start + 1 - term.len();
            return Some(score - GAP_PENALTY * gaps as i32);
        }
    }
    None
}

fn is_boundary(haystack: &[char], position: usize) -> bool {
    match position {
        0 => true,
        position => matches!(
            haystack[position - 1],
            ' ' | '-' | '_' | '/' | '.' | ':' | ',' | '@'
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(haystack: &str, query: &str) -> i32 {
        score(haystack, query).unwrap_or_else(|| panic!("{query} did not match {haystack}"))
    }

    #[test]
    fn an_abbreviation_still_reaches_the_whole_name() {
        assert!(score("ssh-key", "sk").is_some());
        assert!(score("database url", "db").is_some());
        assert!(score("生产环境 令牌", "生产").is_some());
        assert!(score("ed25519", "e9").is_some());
    }

    #[test]
    fn characters_must_stay_in_order() {
        assert!(score("ssh-key", "ks").is_none());
        assert!(score("monica", "monicat").is_none());
    }

    #[test]
    fn every_term_has_to_hit() {
        assert!(score("Monica Keys/ed25519 demo", "keys demo").is_some());
        assert!(score("Monica Keys/ed25519 demo", "keys absent").is_none());
    }

    #[test]
    fn a_contiguous_hit_beats_the_same_letters_spread_out() {
        assert!(term("api-token", "token") > term("t-o-k-e-n notes", "token"));
    }

    #[test]
    fn a_hit_at_a_word_start_beats_one_mid_word() {
        assert!(term("ssh key", "key") > term("donkey stable", "key"));
    }

    #[test]
    fn a_shorter_gap_between_matches_costs() {
        assert!(term("abcdef", "acf") > term("axxxbcxxxf", "acf"));
    }

    #[test]
    fn matching_is_case_insensitive_on_both_sides() {
        assert_eq!(term("SSH-KEY", "sk"), term("ssh-key", "SK"));
    }

    #[test]
    fn an_empty_query_matches_everything_with_no_score() {
        assert_eq!(score("anything", ""), Some(0));
        assert_eq!(score("anything", "   "), Some(0));
    }

    #[test]
    fn several_terms_accumulate() {
        assert!(term("ssh key file", "ssh key") > term("ssh key file", "ssh"));
    }

    #[test]
    fn a_later_word_boundary_start_can_win_the_alignment() {
        assert_eq!(
            term("xkey ssh key", "key"),
            MATCH * 3 + BONUS_ANCHOR + BONUS_CONSECUTIVE * 2
        );
    }
}
