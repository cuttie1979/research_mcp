//! Web search — multi-backend with fallback.
//! Tiers (all keyless HTML scrapers):
//!   1. DuckDuckGo HTML (primary)
//!   2. Bing HTML (secondary)
//!   3. Brave HTML (tertiary fallback)
//! Each tier detects bot-gating and yields to the next; a retry/backoff
//! pass covers transient gating before the query gives up.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::Result;
use regex::Regex;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

    /// Run a query over all backends with fallback + retry.
    /// Order: DDG → Bing → Brave. A backend counts as "usable" iff it parses
    /// to at least one result; empty/gated results fall through to the next.
    /// If all tiers return nothing (e.g. transient bot-gating), retry the whole
    /// chain with short backoff before giving up — so an intermittent challenge
    /// doesn't silently produce zero web results.
    pub async fn query(&self, query: &str, max: usize) -> Result<Vec<SearchResult>> {
        for attempt in 0..3 {
            let results = self.try_all_backends(query, max).await;
            if !results.is_empty() {
                if attempt > 0 {
                    log_info!("  ✓ web search recovered after {attempt} retr(ies)");
                }
                return Ok(results);
            }
            if attempt < 2 {
                log_warn!("  ⚠ all web backends empty/gated for {query:?}; retry {} in 2s...", attempt + 1);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
        log_warn!("  ⚠ web search returned 0 results for {query:?} after 3 backend sweeps (DDG/Bing/Brave gated or empty)");
        Ok(Vec::new())
    }

    /// Try each backend in order, returning the first non-empty result set.
    async fn try_all_backends(&self, query: &str, max: usize) -> Vec<SearchResult> {
        // 1. DuckDuckGo
        match self.ddg(query, max).await {
            Ok(r) if !r.is_empty() => return r,
            Ok(_) => log_warn!("  ⚠ DDG empty/gated for {query:?}"),
            Err(e) => log_warn!("  ⚠ DDG error: {e}"),
        }
        // 2. Bing
        match self.bing(query, max).await {
            Ok(r) if !r.is_empty() => return r,
            Ok(_) => log_warn!("  ⚠ Bing empty/gated for {query:?}"),
            Err(e) => log_warn!("  ⚠ Bing error: {e}"),
        }
        // 3. Brave (last resort)
        match self.brave(query, max).await {
            Ok(r) => return r,
            Err(e) => log_warn!("  ⚠ Brave error: {e}"),
        }
        Vec::new()
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
        if is_bot_gated(&html, &["anomaly", "challenge-form"]) {
            return Ok(Vec::new());
        }

        let results = parse_ddg(&html, max)?;
        Ok(results)
    }

    /// Bing HTML endpoint (keyless). Returns a fresh cookie jar each call so
    /// gating cookies from another engine don't poison the request.
    async fn bing(&self, query: &str, max: usize) -> Result<Vec<SearchResult>> {
        let client = reqwest::Client::builder()
            .cookie_store(true)
            .build()
            .expect("build bing client");

        let html = client
            .get("https://www.bing.com/search")
            .query(&[("q", query), ("count", "10")])
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36")
            .header("Accept-Language", "en-US,en;q=0.9")
            .send()
            .await?
            .text()
            .await?;

        let results = parse_bing(&html, max)?;

        // Only treat as gated if there are NO results AND a real challenge is
        // present. Bing's normal page contains "challenges.cloudflare.com" /
        // "PoWChallengeSolver" in its JS config, so an over-broad "challenge"
        // marker would false-positive and throw away real results.
        if results.is_empty()
            && is_bot_gated(&html, &["captcha", "unusual traffic", "verify you're not a robot"])
        {
            return Ok(Vec::new());
        }

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

        // Only treat as gated if no results AND a real challenge marker present.
        if results.is_empty()
            && is_bot_gated(&html, &["captcha", "verify you're not a robot", "unusual traffic"])
        {
            return Ok(Vec::new());
        }

        Ok(results)
    }
}

/// True if the page is a bot/anomaly challenge rather than a normal SERP.
/// Case-insensitive substring match on any of the given markers.
fn is_bot_gated(html: &str, markers: &[&str]) -> bool {
    let lower = html.to_ascii_lowercase();
    markers.iter().any(|m| lower.contains(&m.to_ascii_lowercase()))
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

/// Parse Bing HTML result blocks.
/// Structure:
/// <li class="b_algo">
///   <h2 class=""><a target="_blank" href="https://www.bing.com/ck/a?...&u=BASE64URL...">
///     TITLE
///   </a></h2>
///   <p class="...b_lineclamp...">SNIPPET</p>
/// The result URL is a `bing.com/ck/a` redirect with the real URL in its
/// base64url `u=` parameter; decode that, else fall back to the raw href.
fn parse_bing(html: &str, max: usize) -> Result<Vec<SearchResult>> {
    let block_re = Regex::new(r#"<li class="b_algo""#)?;
    let blocks: Vec<&str> = block_re.split(html).skip(1).collect();

    let link_re = Regex::new(r#"<h2[^>]*><a[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#)?;
    let snippet_re = Regex::new(r#"<p[^>]*class="[^"]*b_lineclamp[^"]*"[^>]*>(.*?)</p>"#)?;
    let u_re = Regex::new(r#"[?&]u=([^&]+)"#)?;

    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for block in blocks.into_iter().take(max * 2) {
        if out.len() >= max {
            break;
        }
        let Some(cap) = link_re.captures(block) else { continue };
        let raw_href = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
        let title = strip_html(cap.get(2).map(|m| m.as_str()).unwrap_or_default());

        // Decode the real URL from the ck/a redirect's base64url `u=` param.
        let mut url = raw_href.trim().to_string();
        if let Some(u) = u_re.captures(&raw_href.replace("&amp;", "&"))
            .and_then(|c| c.get(1)) {
            let b64 = u.as_str().to_string();
            if let Some(decoded) = base64url_decode(&b64) {
                url = decoded;
            }
        }

        if url.is_empty() || url.starts_with("bing.com") {
            continue;
        }
        if !seen.insert(url.clone()) {
            continue;
        }

        let snippet = snippet_re
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| strip_html(m.as_str()))
            .unwrap_or_default();

        out.push(SearchResult { title, url, snippet });
    }

    Ok(out)
}

/// Decode a Bing base64url value (URL-safe alphabet, padding optional).
/// Bing prefixes its `u=` param with "a1" (a NodeJS Buffer/encoding marker);
/// strip it before decoding, since it is not part of the base64 payload.
fn base64url_decode(s: &str) -> Option<String> {
    use base64::Engine;
    let payload = s.strip_prefix("a1").unwrap_or(s);
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let bytes = engine.decode(payload.as_bytes()).ok()?;
    String::from_utf8(bytes).ok()
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
    #[ignore = "live search test — external engines rate-limit bursty traffic; run with --ignored"]
    async fn live_search_with_fallback() {
        // External search engines rate-limit bursty test traffic; retry a few times.
        let s = Search::new();
        let mut last_err: Option<String> = None;
        for attempt in 0..4 {
            match s.query("Rust programming language", 5).await {
                Ok(results) if !results.is_empty() => {
                    for r in results.iter().take(3) {
                        assert!(!r.url.is_empty());
                        assert!(!r.title.is_empty());
                        log_info!("  {} — {}", r.title, r.url);
                    }
                    return;
                }
                Ok(_) => last_err = Some("empty results".into()),
                Err(e) => last_err = Some(e.to_string()),
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
        panic!("search failed after retries: {}", last_err.unwrap_or_default());
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

    #[tokio::test]
    #[ignore = "live — run manually: cargo test -- --ignored search_live_energy_news"]
    async fn search_live_energy_news() {
        // Reproduces the reported bug: "latest news energy market past week"
        // returned zero web results. Exercises the real DDG->Brave fallback chain.
        let s = Search::new();
        let q = "latest news on the energy market over the past one week";
        let res = s.query(q, 6).await.expect("query should not error");
        log_info!("  query: {q}");
        log_info!("  results: {}", res.len());
        for r in res.iter().take(8) {
            log_info!("    - {} | {}", r.title, r.url);
        }
        assert!(!res.is_empty(), "web search returned no results — bug reproduces");
    }

    #[test]
    fn parse_bing_sample() {
        // Bing uses `b_algo` blocks with a `ck/a` redirect carrying the real
        // URL base64url in its `u=` param.
        let html = r#"<ol id="b_results"><li class="b_algo">
            <h2 class=""><a target="_blank" href="https://www.bing.com/ck/a?!&amp;&amp;p=abc&amp;u=aHR0cHM6Ly9leGFtcGxlLmNvbS9wYWdl&amp;ntb=1">Example Page</a></h2>
            <p class="b_lineclamp">Here is the <strong>snippet</strong> text.</p>
        </li></ol>"#;
        let results = parse_bing(html, 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example Page");
        assert_eq!(results[0].url, "https://example.com/page");
        assert!(results[0].snippet.contains("snippet text"));
    }

    #[test]
    fn base64url_decode_handles_missing_padding_and_a1_prefix() {
        // No padding: aHR0cHM6Ly9leGFtcGxlLmNvbS9wYWdl == "https://example.com/page"
        assert_eq!(
            base64url_decode("aHR0cHM6Ly9leGFtcGxlLmNvbS9wYWdl").as_deref(),
            Some("https://example.com/page")
        );
        // Real Bing value carries a leading "a1" marker that must be stripped:
        // a1aHR0cHM6Ly9uaWxlcG9zdC5jby51Zy8 == "https://nilepost.co.ug/"
        assert_eq!(
            base64url_decode("a1aHR0cHM6Ly9uaWxlcG9zdC5jby51Zy8").as_deref(),
            Some("https://nilepost.co.ug/")
        );
    }

    #[test]
    fn bot_gating_detection() {
        assert!(is_bot_gated("<html>anomaly detected</html>", &["anomaly", "challenge-form"]));
        assert!(is_bot_gated("<html>Complete CAPTCHA to continue</html>", &["captcha", "challenge"]));
        assert!(!is_bot_gated("<html>normal results here</html>", &["captcha", "anomaly"]));
    }
}

#[cfg(test)]
mod bing_live_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "live — run manually: cargo test -- --ignored bing_live"]
    async fn bing_live() {
        // Verifies the Bing tier independently returns results (so that if DDG
        // is gated, the new fallback tier rescues the query).
        let s = Search::new();
        let res = s.bing("latest news energy market", 8).await.expect("bing should not error");
        log_info!("  bing results: {}", res.len());
        for r in res.iter().take(5) {
            log_info!("    - {} | {}", r.title, r.url);
        }
        assert!(!res.is_empty(), "bing backend returned nothing");
    }
}
