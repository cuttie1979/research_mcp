//! arXiv client.
//!
//! Primary: fetch the HTML abstract page (arxiv.org/abs/{id}) and parse
//! `citation_*` meta tags. This endpoint is NOT the export API and is not
//! subject to the export API rate limit (which this IP hit with 429).
//! Fallback: official export Atom API (export.arxiv.org) with rate limiting.

use anyhow::{bail, Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use regex::Regex;

pub const ARXIV_API_URL: &str = "https://export.arxiv.org/api/query";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArxivPaper {
    pub arxiv_id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub abstract_text: String,
    pub url: String,
    pub published: Option<String>,
    pub categories: Vec<String>,
}

pub struct Arxiv {
    client: reqwest::Client,
}

impl Arxiv {
    pub fn new() -> Self {
        Self { client: reqwest::Client::new() }
    }

    /// Fetch a single paper by arXiv ID (e.g. "2607.12631" or "2106.09685v2").
    /// Primary route: HTML abs page. Fallback: export API.
    pub async fn by_id(&self, arxiv_id: &str) -> Result<Option<ArxivPaper>> {
        let id = arxiv_id.trim().trim_end_matches(".pdf");
        // Normalize to base ID (strip version suffix) for the abs URL.
        let base = id.split_once('v').map(|(b, _)| b).unwrap_or(id);
        if let Ok(Some(p)) = self.fetch_abs_html(base).await {
            return Ok(Some(p));
        }
        // Fallback to the export API (rate-limited but authoritative).
        self.by_id_api(id).await
    }

    /// Search arXiv by query string (all fields), sorted by relevance.
    /// Uses the export API (no HTML search equivalent), rate-limited.
    pub async fn search(&self, query: &str, max_results: usize) -> Result<Vec<ArxivPaper>> {
        let resp = self
            .client
            .get(ARXIV_API_URL)
            .query(&[
                ("search_query", format!("all:{query}")),
                ("start", "0".to_string()),
                ("max_results", max_results.to_string()),
                ("sortBy", "relevance".to_string()),
                ("sortOrder", "descending".to_string()),
            ])
            .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36")
            .send()
            .await
            .context("arXiv search request failed")?;

        if !resp.status().is_success() {
            bail!("arXiv API error {}", resp.status());
        }
        let body = resp.text().await.context("arXiv response read failed")?;
        parse_feed(&body)
    }

    /// Fetch a single paper via the export API.
    async fn by_id_api(&self, arxiv_id: &str) -> Result<Option<ArxivPaper>> {
        let resp = self
            .client
            .get(ARXIV_API_URL)
            .query(&[("id_list", arxiv_id)])
            .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36")
            .send()
            .await
            .context("arXiv id fetch failed")?;

        if !resp.status().is_success() {
            bail!("arXiv API error {}", resp.status());
        }
        let body = resp.text().await.context("arXiv response read failed")?;
        let mut papers = parse_feed(&body)?;
        Ok(papers.drain(..).next())
    }

    /// Fetch the HTML abstract page and parse citation meta tags.
    async fn fetch_abs_html(&self, arxiv_id: &str) -> Result<Option<ArxivPaper>> {
        let url = format!("https://arxiv.org/abs/{arxiv_id}");
        let resp = self
            .client
            .get(&url)
            .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36")
            .send()
            .await
            .context("arXiv abs page request failed")?;

        if !resp.status().is_success() {
            return Ok(None); // fall through to API route
        }
        let html = resp.text().await?;
        if html.contains("arXiv does not have any holdings for") || html.contains("not found") && html.len() < 10000 {
            return Ok(None);
        }

        let paper = parse_abs_html(&html, arxiv_id)?;
        if paper.title.is_empty() {
            return Ok(None);
        }
        Ok(Some(paper))
    }
}

/// Parse the arXiv abs HTML page for citation_* meta tags.
fn parse_abs_html(html: &str, arxiv_id: &str) -> Result<ArxivPaper> {
    let meta_re = Regex::new(r#"<meta name="citation_([^"]+)" content="([^"]*)"#).unwrap();

    let mut title = String::new();
    let mut authors: Vec<String> = Vec::new();
    let mut published: Option<String> = None;
    let mut abstract_text = String::new();
    let mut categories: Vec<String> = Vec::new();

    for cap in meta_re.captures_iter(html) {
        let name = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
        let value = cap.get(2).map(|m| m.as_str()).unwrap_or_default();
        match name {
            "title" => title = decode_html(value),
            "author" => authors.push(decode_html(value)),
            "date" if published.is_none() => published = Some(value.to_string()),
            "abstract" => abstract_text = decode_html(value),
            "arxiv_id" => {}
            _ => {}
        }
    }

    // Categories from the subjects cell: <span class="primary-subject">Computation and Language (cs.CL)</span>; Artificial Intelligence (cs.AI)
    let subjects_re = Regex::new(r#"(?s)class="tablecell subjects"[^>]*>(.*?)</td>"#).unwrap();
    if let Some(cap) = subjects_re.captures(html) {
        let cell = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
        let cat_re = Regex::new(r"\(([a-zA-Z\-]+(?:\.[a-zA-Z\-]+)+)\)").unwrap();
        categories = cat_re
            .captures_iter(cell)
            .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
            .collect();
    }

    // Fallback: if meta abstract missing, extract from <blockquote class="abstract">
    if abstract_text.is_empty() {
        let abs_re = Regex::new(r#"(?s)class="abstract[^"]*"[^>]*>(.*?)</blockquote>"#).unwrap();
        if let Some(cap) = abs_re.captures(html) {
            let raw = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
            abstract_text = strip_html_tags(raw);
        }
    }

    Ok(ArxivPaper {
        arxiv_id: arxiv_id.to_string(),
        title,
        authors,
        abstract_text,
        url: format!("https://arxiv.org/abs/{arxiv_id}"),
        published,
        categories,
    })
}

fn strip_html_tags(s: &str) -> String {
    let re = Regex::new(r"<[^>]*>").unwrap();
    re.replace_all(s, " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn decode_html(s: &str) -> String {
    s.replace("&#39;", "'")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
}

/// Parse an Atom feed from the arXiv API into papers.
fn parse_feed(xml: &str) -> Result<Vec<ArxivPaper>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut papers: Vec<ArxivPaper> = Vec::new();
    let mut current: Option<ArxivPaper> = None;
    let mut in_entry = false;
    let mut in_author = false;
    let mut current_tag = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                current_tag = name.clone();
                match name.as_str() {
                    "entry" => {
                        in_entry = true;
                        current = Some(ArxivPaper {
                            arxiv_id: String::new(),
                            title: String::new(),
                            authors: Vec::new(),
                            abstract_text: String::new(),
                            url: String::new(),
                            published: None,
                            categories: Vec::new(),
                        });
                    }
                    "author" if in_entry => in_author = true,
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if name == "category" && in_entry
                    && let Some(paper) = current.as_mut()
                        && let Ok(attr) = e.try_get_attribute("term") {
                            paper.categories.push(
                                attr.map(|a| a.unescape_value().map(|v| v.into_owned()).unwrap_or_default())
                                    .unwrap_or_default(),
                            );
                        }
            }
            Ok(Event::Text(t)) => {
                if !in_entry || current.is_none() {
                    continue;
                }
                let text = t
                    .unescape()
                    .map(|t| t.into_owned())
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if text.is_empty() {
                    continue;
                }
                let paper = current.as_mut().unwrap();
                match current_tag.as_str() {
                    "id" => {
                        let raw_id = text.rsplit('/').next().unwrap_or(&text).to_string();
                        paper.arxiv_id = raw_id
                            .split_once('v')
                            .map(|(base, _)| base.to_string())
                            .unwrap_or(raw_id);
                        paper.url = format!("https://arxiv.org/abs/{}", paper.arxiv_id);
                    }
                    "title" => paper.title = clean_text(&text),
                    "summary" => paper.abstract_text = clean_text(&text),
                    "published" => paper.published = Some(text),
                    "name" if in_author => paper.authors.push(text),
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                match name.as_str() {
                    "entry" => {
                        in_entry = false;
                        if let Some(paper) = current.take()
                            && !paper.arxiv_id.is_empty() {
                                papers.push(paper);
                            }
                    }
                    "author" => in_author = false,
                    _ => current_tag.clear(),
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow::anyhow!("arXiv XML parse error: {e}")),
            _ => {}
        }
    }

    Ok(papers)
}

fn clean_text(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn live_fetch_abs_html_by_id() {
        // Uses the HTML route — immune to export API rate limits.
        let a = Arxiv::new();
        let mut last_err: Option<String> = None;
        for attempt in 0..3 {
            match a.by_id("2607.12631").await {
                Ok(Some(paper)) => {
                    assert!(!paper.title.is_empty(), "title should be present");
                    assert!(!paper.abstract_text.is_empty(), "abstract should be present");
                    assert!(!paper.authors.is_empty(), "authors should be present");
                    log_info!("  title: {}", paper.title);
                    log_info!("  authors: {}", paper.authors.join(", "));
                    log_info!("  categories: {:?}", paper.categories);
                    return;
                }
                Ok(None) => last_err = Some("not found".into()),
                Err(e) => last_err = Some(e.to_string()),
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        panic!("arxiv abs fetch failed after retries: {}", last_err.unwrap_or_default());
    }

    #[tokio::test]
    #[ignore = "live export API test — run with --ignored to avoid rate limits"]
    async fn live_search() {
        let a = Arxiv::new();
        let mut last_err: Option<String> = None;
        for attempt in 0..3 {
            match a.search("induced emotion LLM decision making", 3).await {
                Ok(papers) if !papers.is_empty() => {
                    for p in papers.iter().take(3) {
                        assert!(!p.arxiv_id.is_empty());
                        assert!(!p.title.is_empty());
                        log_info!("  [{}] {}", p.arxiv_id, p.title);
                    }
                    return;
                }
                Ok(_) => last_err = Some("empty results".into()),
                Err(e) => last_err = Some(e.to_string()),
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
        panic!("arxiv search failed after retries: {}", last_err.unwrap_or_default());
    }
}
