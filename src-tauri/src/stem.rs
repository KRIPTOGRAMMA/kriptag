// Stemming for note search: reduces a word to its stem so that a query written
// in one form finds the text written in another — "покупки" finds "покупками".
//
// Pure functions only, no database: the whole point is that this is testable on
// its own, and the indexing side (commands/notes.rs) just calls stem_text.
//
// What this does NOT fix, verified before it was chosen and kept here so the
// behaviour is not later mistaken for a bug:
//   "покупок" -> "покупок"  (a fleeting vowel; Snowball will not reduce it the
//                            way it reduces "покупки" to "покупк")
//   "бежать" -> "бежа"  vs  "бегу" -> "бег"  (suppletive stems are out of reach
//                            for any suffix-stripping algorithm)
// Some forms will not meet even with stemming. Closing that gap is what
// embeddings are for, and they are a separate decision with a separate cost.

use rust_stemmers::{Algorithm, Stemmer};

/// Picks the stemmer per word by script rather than by the interface language.
///
/// Notes are mixed in practice ("deploy на проде"), and the language of the UI
/// says nothing about the language of a given word. Going by the UI setting
/// would run the Russian algorithm over English words, producing stems that
/// match nothing.
fn is_cyrillic_word(word: &str) -> bool {
    word.chars().any(|c| matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё'))
}

/// Stems one word. Case is folded first: the index and the query have to agree,
/// and the user does not type the same capitalisation twice.
pub fn stem_word(word: &str) -> String {
    let lower = word.to_lowercase();
    if lower.is_empty() {
        return lower;
    }

    let algorithm = if is_cyrillic_word(&lower) {
        Algorithm::Russian
    } else {
        Algorithm::English
    };

    Stemmer::create(algorithm).stem(&lower).to_string()
}

/// Stems every word of a text, keeping them in order and separated by spaces.
///
/// Word boundaries are anything that is not alphanumeric, so punctuation and
/// markdown syntax fall away on their own. The result is fed to FTS5, which
/// tokenizes it again — it only has to be words separated by spaces, not
/// readable text.
pub fn stem_text(text: &str) -> String {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(stem_word)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn russian_forms_of_one_word_share_a_stem() {
        let stem = stem_word("покупки");
        assert_eq!(stem_word("покупка"), stem);
        assert_eq!(stem_word("покупками"), stem);
    }

    #[test]
    fn english_forms_of_one_word_share_a_stem() {
        let stem = stem_word("running");
        assert_eq!(stem_word("runs"), stem);
        assert_eq!(stem_word("run"), stem);
    }

    #[test]
    fn script_decides_the_algorithm_word_by_word() {
        // The English stemmer would leave "созвоны" alone, the Russian one would
        // mangle "meetings" — a mixed line has to get both right at once.
        let mixed = stem_text("meetings и созвоны");
        assert!(mixed.contains(&stem_word("созвон")), "russian word not stemmed: {mixed}");
        assert!(mixed.contains(&stem_word("meeting")), "english word not stemmed: {mixed}");
    }

    #[test]
    fn case_is_folded_so_index_and_query_agree() {
        assert_eq!(stem_word("ПОКУПКИ"), stem_word("покупки"));
        assert_eq!(stem_word("Running"), stem_word("running"));
    }

    #[test]
    fn punctuation_and_markdown_are_not_words() {
        assert_eq!(stem_text("**списк**, покупок!"), format!("{} {}", stem_word("списк"), stem_word("покупок")));
    }

    #[test]
    fn empty_input_yields_empty_output() {
        assert_eq!(stem_text(""), "");
        assert_eq!(stem_text("  ...  "), "");
        assert_eq!(stem_word(""), "");
    }

    #[test]
    fn digits_survive_as_words() {
        // A note saying "отчёт за 2026" must still be findable by "2026".
        assert!(stem_text("отчёт за 2026").contains("2026"));
    }

    // The two known limits, pinned deliberately. If a later change makes these
    // pass, that is an improvement — but it must be a deliberate one, not a
    // silent shift in what search finds.
    #[test]
    fn known_limit_fleeting_vowel_does_not_reduce() {
        assert_ne!(
            stem_word("покупок"), stem_word("покупки"),
            "если беглая гласная стала сводиться — это улучшение, но осознанное: обнови план"
        );
    }

    #[test]
    fn known_limit_suppletive_stems_stay_apart() {
        assert_ne!(
            stem_word("бежать"), stem_word("бегу"),
            "супплетивные основы алгоритму не даются; лечится только словарём или эмбеддингами"
        );
    }
}
