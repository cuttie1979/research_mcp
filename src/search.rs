//! Web search — multi-backend with fallback.
//! Primary: DuckDuckGo HTML (no key). Fallback: Brave HTML (no key).

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
        Self {
            client: reqwest::Client::builder()
                .cookie_store(true)
                .build()
                .expect("build http client"),
        }
    }

    /// Run a query. Tries DuckDuckGo first; falls back to Brave if DDG
    /// returns no results or a challenge/anomaly page.
    pub async fn query(&self, query: &str, max: usize) -> Result<Vec<SearchResult>> {
        let ddg = self.ddg(query, max).await?;
        if !ddg.is_empty() {
            return Ok(ddg);
        }
        eprintln!("  ⚠ DDG returned nothing for {query:?}, trying Brave...");
        self.brave(query, max).await
    }

    /// DuckDuckGo HTML endpoint.
    async fn ddg(&self, query: &str, max: usize) -> Result<Vec<SearchResult>> {
        let html = self
            .client
            .get("https://html.duckduckgo.com/html/")
            .query(&[("q", query)])
            .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36")
            .send()
            .await?
            .text()
            .await?;

        // If the page is an anomaly/challenge, return empty to trigger fallback.
        if html.contains("anomaly") || html.contains("challenge-form") {
            return Ok(Vec::new());
        }

        let results = parse_ddg(&html, max)?;
        Ok(results)
    }

    /// Brave search HTML endpoint. Uses its own client with a fresh cookie
    /// jar (DDG anomaly cookies would otherwise poison the request).
    /// Warms up the home page first to acquire anti-bot cookies.
    async fn brave(&self, query: &str, max: usize) -> Result<Vec<SearchResult>> {
        let client = reqwest::Client::builder()
            .cookie_store(true)
            .build()
            .expect("build brave client");

        let _ = client
            .get("https://search.brave.com/")
            .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36")
            .send()
            .await?;

        let html = client
            .get("https://search.brave.com/search")
            .query(&[("q", query)])
            .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36")
            .header("Accept-Language", "en-US,en;q=0.9")
            .send()
            .await?
            .text()
            .await?;

        let results = parse_brave(&html, max)?;
        Ok(results)
    }
}

/// Parse DuckDuckGo HTML result blocks.
fn parse_ddg(html: &str, max: usize) -> Result<Vec<SearchResult>> {
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

/// Parse Brave search result blocks.
/// Structure (SvelteKit HTML):
/// <div class="snippet svelte-jmfu5f" data-pos="N" data-type="web">
///   <a href="URL" class="...">
///     <div class="title ..." title="TITLE">TITLE</div>
///   </a>
///   <div class="content ...">SNIPPET</div>
fn parse_brave(html: &str, max: usize) -> Result<Vec<SearchResult>> {
    // Split into per-result blocks by the snippet container.
    let block_re = Regex::new(
        r#"<div class="snippet svelte-jmfu5f" data-pos="\d+" data-type="web""#,
    )?;
    let blocks: Vec<&str> = block_re.split(html).skip(1).collect();

    let title_re = Regex::new(r#"<div class="title[^"]*"[^>]*title="([^"]+)"[^>]*>"#)?;
    let url_re = Regex::new(r#"<a href="(https?://[^"]+)"[^>]*class="[^"]*l1[^"]*""#)?;
    let snippet_re = Regex::new(
        r#"(?s)<div class="content desktop-default-regular[^"]*"[^>]*>(.*?)</div>"#,
    )?;

    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for block in blocks.into_iter().take(max * 2) {
        if out.len() >= max {
            break;
        }
        let title = title_re
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        let url = url_re
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();

        if url.is_empty() || title.is_empty() {
            continue;
        }
        if !seen.insert(url.clone()) {
            continue;
        }

        // Snippet: content div, may contain nested tags — strip them.
        let snippet = snippet_re
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| strip_html(m.as_str()))
            .unwrap_or_default();

        out.push(SearchResult { title, url, snippet });
    }

    Ok(out)
}

fn urlencoding_decode(s: &str) -> String {
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
    // Remove HTML comments first (they contain > chars which break tag stripping).
    let re_comment = Regex::new(r"(?s)<!--.*?-->").unwrap();
    let no_comments = re_comment.replace_all(s, " ");
    let re = Regex::new(r"<[^>]*>").unwrap();
    let cleaned = re.replace_all(&no_comments, " ");
    cleaned
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn live_search_with_fallback() {
        let s = Search::new();
        let results = s.query("Rust programming language", 5).await.expect("search should work");
        assert!(!results.is_empty(), "expected some results from DDG or Brave");
        for r in results.iter().take(3) {
            assert!(!r.url.is_empty());
            assert!(!r.title.is_empty());
            println!("  {} — {}", r.title, r.url);
        }
    }

    #[test]
    fn parse_brave_sample() {
        let html = r#"
        <div class="snippet svelte-jmfu5f" data-pos="1" data-type="web">
          <div class="result-content svelte-1rq4ngz">
            <a href="https://example.com/page" target="_self" class="svelte-14r20fy l1">
              <div class="title search-snippet-title line-clamp-1 svelte-14r20fy" title="Example Page">Example Page</div>
            </a>
            <div class="generic-snippet svelte-1cwdgg3">
              <div class="content desktop-default-regular t-primary line-clamp-dynamic svelte-1cwdgg3">
                This is the <strong>snippet</strong> text.
              </div>
            </div>
          </div>
        </div>"#;
        let results = parse_brave(html, 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example Page");
        assert_eq!(results[0].url, "https://example.com/page");
        assert!(results[0].snippet.contains("snippet text"));
    }
}
