//! Slug generation from a topic: lowercase, hyphenated, no filler words, max 5 words.

const FILLER_WORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "of", "for", "in", "on", "with", "to", "from", "at", "by",
    "about", "into", "over", "after", "before", "under", "is", "are", "was", "were", "be",
    "what", "why", "how", "when", "where", "who", "which",
];

pub fn slugify(topic: &str) -> String {
    let lower = topic.to_lowercase();

    // Split into words on non-alphanumeric boundaries.
    let words: Vec<String> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|w| !w.is_empty())
        .collect();

    let mut kept: Vec<String> = Vec::new();
    for w in words {
        if kept.len() >= 5 {
            break;
        }
        if FILLER_WORDS.contains(&w.as_str()) {
            continue;
        }
        kept.push(w);
    }

    if kept.is_empty() {
        return "research".to_string();
    }

    kept.join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        assert_eq!(slugify("What is the history of the Roman Empire"), "history-roman-empire");
    }

    #[test]
    fn max_five_words() {
        assert_eq!(
            slugify("A comprehensive analysis of modern transformer architecture scaling laws"),
            "comprehensive-analysis-modern-transformer-architecture"
        );
    }

    #[test]
    fn empty() {
        assert_eq!(slugify("a an the"), "research");
    }
}
