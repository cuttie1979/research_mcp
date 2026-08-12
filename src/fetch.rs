//! URL content fetch — fetch a page and extract readable text.

use anyhow::{bail, Result};
use regex::Regex;

pub struct Fetcher {
    client: reqwest::Client,
}

impl Fetcher {
    pub fn new() -> Self {
        Self { client: reqwest::Client::new() }
    }

    /// Fetch a URL and return cleaned plain text, truncated to `max_chars`.
    pub async fn fetch(&self, url: &str, max_chars: usize) -> Result<String> {
        let resp = self
            .client
            .get(url)
            .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36")
            .header("Accept", "text/html,application/xhtml+xml,text/plain")
            .send()
            .await?;

        if !resp.status().is_success() {
            bail!("HTTP {} for {}", resp.status(), url);
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_lowercase();

        let body = resp.text().await?;

        let text = if content_type.contains("pdf") {
            bail!("PDF content not supported: {}", url);
        } else if content_type.contains("html") || body.trim_start().starts_with('<') {
            html_to_text(&body)
        } else {
            body
        };

        let text = text.trim().to_string();
        if text.chars().count() > max_chars {
            Ok(text.chars().take(max_chars).collect())
        } else {
            Ok(text)
        }
    }
}

fn html_to_text(html: &str) -> String {
    // Remove scripts/styles/nav/footer/header/noscript blocks (no backrefs supported).
    let mut s = html.to_string();
    for tag in ["script", "style", "nav", "footer", "header", "noscript"] {
        let re = Regex::new(&format!(r"(?is)<{tag}[^>]*>.*?</{tag}>")).unwrap();
        s = re.replace_all(&s, " ").into_owned();
    }

    // Block-level tags → newline.
    let re_line = Regex::new(r"(?i)</(p|div|h[1-6]|li|tr|section|article|blockquote|pre)>").unwrap();
    let with_newlines = re_line.replace_all(&s, "\n");

    // Remaining tags → space.
    let re_tag = Regex::new(r"<[^>]*>").unwrap();
    let no_tags = re_tag.replace_all(&with_newlines, " ");

    // Entities.
    let entities = no_tags
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'");

    // Collapse blank lines / whitespace.
    let re_blank = Regex::new(r"[ \t]+\n").unwrap();
    let cleaned = re_blank.replace_all(&entities, "\n");
    let re_multi = Regex::new(r"\n{3,}").unwrap();
    let cleaned = re_multi.replace_all(&cleaned, "\n\n");

    cleaned.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn live_fetch_rustlang() {
        let f = Fetcher::new();
        let text = f.fetch("https://www.rust-lang.org/", 2000).await.expect("fetch should work");
        assert!(text.chars().count() >= 100, "expected meaningful content, got {} chars", text.chars().count());
        log_info!("fetched {} chars, starts with: {}", text.chars().count(), text.chars().take(120).collect::<String>());
    }
}
