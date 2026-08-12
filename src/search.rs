//! DuckDuckGo HTML search — no API key required.

use std::collections::HashSet;

use anyhow::Result;
use regex::Regex;

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

pub struct Search {
    client: reqwest::Client,
}

impl Search {
    pub fn new() -> Self {
        Self { client: reqwest::Client::new() }
    }

    /// Run a query against DuckDuckGo HTML endpoint.
    /// Returns up to `max` unique results (deduped by URL).
    pub async fn query(&self, query: &str, max: usize) -> Result<Vec<SearchResult>> {
        let html = self
            .client
            .get("https://html.duckduckgo.com/html/")
            .query(&[("q", query)])
            .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36")
            .send()
            .await?
            .text()
            .await?;

        let results = parse_results(&html, max)?;
        Ok(results)
    }
}

fn parse_results(html: &str, max: usize) -> Result<Vec<SearchResult>> {
    // DuckDuckGo HTML structure:
    // <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=<urlencoded>&...">Title</a>
    // <a class="result__snippet" ...>Snippet</a>
    let result_re =
        Regex::new(r#"class="result__a"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#)?;
    let snippet_re = Regex::new(r#"class="result__snippet"[^>]*>(.*?)</a>"#)?;
    let uddg_re = Regex::new(r"uddg=([^&]+)")?;

    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for cap in result_re.captures_iter(html) {
        if out.len() >= max {
            break;
        }
        let raw_url = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
        let title = strip_html(cap.get(2).map(|m| m.as_str()).unwrap_or_default());

        // Decode real URL from duckduckgo redirect param, if present.
        let url = if let Some(u) = uddg_re.captures(raw_url).and_then(|c| c.get(1)) {
            urlencoding_decode(u.as_str())
        } else {
            raw_url.trim_start_matches("//").to_string()
        };

        if url.is_empty() || url.starts_with("duckduckgo.com") {
            continue;
        }
        if !seen.insert(url.clone()) {
            continue;
        }

        let snippet = snippet_re
            .captures_iter(html)
            .nth(out.len())
            .and_then(|c| c.get(1))
            .map(|m| strip_html(m.as_str()))
            .unwrap_or_default();

        out.push(SearchResult { title, url, snippet });
    }

    Ok(out)
}

fn urlencoding_decode(s: &str) -> String {
    // Percent-decoding, keep it dependency-free.
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn strip_html(s: &str) -> String {
    let re = Regex::new(r"<[^>]*>").unwrap();
    let cleaned = re.replace_all(s, " ");
    cleaned
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn live_duckduckgo_search() {
        let s = Search::new();
        let results = s.query("Rust programming language", 5).await.expect("search should work");
        assert!(!results.is_empty(), "expected some results");
        for r in results.iter().take(3) {
            assert!(!r.url.is_empty());
            assert!(!r.title.is_empty());
            println!("  {} — {}", r.title, r.url);
        }
    }
}
