//! Slug generation from a topic: lowercase, hyphenated, no filler words, max 5 words.
//! URLs (http/https/www/arxiv.org/pubmed) and paper IDs (arXiv, PMID) are stripped
//! before tokenizing so they never leak into the slug.

use regex::Regex;

const FILLER_WORDS: &[&str] = &[
    // articles, conjunctions, prepositions
    "a", "an", "the", "and", "or", "of", "for", "in", "on", "with", "to", "from", "at", "by",
    "about", "into", "over", "after", "before", "under", "is", "are", "was", "were", "be",
    // question words
    "what", "why", "how", "when", "where", "who", "which",
    // structural / task words
    "research", "study", "paper", "article", "arxiv", "pubmed", "pmid", "include",
    "including", "using", "used", "based", "it", "its", "them", "their", "this", "that",
    "does", "do", "can", "could", "would", "should", "may", "might", "not",
];

pub fn slugify(topic: &str) -> String {
    let cleaned = strip_noise(topic);
    let lower = cleaned.to_lowercase();

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

/// Remove URLs, arXiv IDs, and PMIDs before tokenization.
fn strip_noise(topic: &str) -> String {
    let mut s = topic.to_string();

    // Full URLs.
    let url_re = Regex::new(r"(?i)https?://[^\s]+").unwrap();
    s = url_re.replace_all(&s, " ").into_owned();

    // Domain-only references like arxiv.org/abs/2607.12631 (after http strip, domain remains).
    let domain_re = Regex::new(r"(?i)(www\.)?(arxiv\.org|pubmed\.ncbi\.nlm\.nih\.gov)[^\s]*").unwrap();
    s = domain_re.replace_all(&s, " ").into_owned();

    // arXiv IDs (e.g. 2607.12631, 2106.09685v2).
    let arxiv_id_re = Regex::new(r"\b\d{4}\.\d{4,5}(?:v\d+)?\b").unwrap();
    s = arxiv_id_re.replace_all(&s, " ").into_owned();

    // PMIDs (e.g. 11504948) — 6-9 digit numbers, avoid stripping years.
    let pmid_re = Regex::new(r"(?i)PMID:?\s*\d{6,9}\b").unwrap();
    s = pmid_re.replace_all(&s, " ").into_owned();

    s
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
    fn strips_url() {
        assert_eq!(
            slugify("Research the arxiv paper https://arxiv.org/abs/2607.12631 — what it is about, its contributions, methods"),
            "contributions-methods"
        );
    }

    #[test]
    fn strips_pmid() {
        assert_eq!(
            slugify("Research the effect of metacognitive training in schizophrenia using the IGT paradigm — include the classic Bechara paper PMID:11504948"),
            "effect-metacognitive-training-schizophrenia-igt"
        );
    }

    #[test]
    fn strips_bare_arxiv_id() {
        assert_eq!(slugify("Analysis of arxiv 2607.12631 methods"), "analysis-methods");
    }

    #[test]
    fn empty() {
        assert_eq!(slugify("a an the"), "research");
    }

    #[test]
    fn only_url() {
        assert_eq!(slugify("https://arxiv.org/abs/2607.12631"), "research");
    }
}
