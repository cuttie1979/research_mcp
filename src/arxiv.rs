//! arXiv API client — official Atom API, no API key required.
//! Endpoint: http://export.arxiv.org/api/query

use anyhow::{bail, Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;

pub const ARXIV_API_URL: &str = "http://export.arxiv.org/api/query";

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

    /// Search arXiv by query string (all fields), sorted by relevance.
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
            .header("User-Agent", "research_mcp/0.1 (Rust deepresearch tool)")
            .send()
            .await
            .context("arXiv search request failed")?;

        if !resp.status().is_success() {
            bail!("arXiv API error {}", resp.status());
        }
        let body = resp.text().await.context("arXiv response read failed")?;
        parse_feed(&body)
    }

    /// Fetch a single paper by arXiv ID (e.g. "2607.12631" or "2106.09685v2").
    pub async fn by_id(&self, arxiv_id: &str) -> Result<Option<ArxivPaper>> {
        let resp = self
            .client
            .get(ARXIV_API_URL)
            .query(&[("id_list", arxiv_id)])
            .header("User-Agent", "research_mcp/0.1 (Rust deepresearch tool)")
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
}

/// Parse an Atom feed from the arXiv API into papers.
fn parse_feed(xml: &str) -> Result<Vec<ArxivPaper>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut papers: Vec<ArxivPaper> = Vec::new();
    let mut current: Option<ArxivPaper> = None;

    // Track the current element context: we care about entry > title/summary/author/name/id/published/category.
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
                if name == "category" && in_entry {
                    if let Some(paper) = current.as_mut() {
                        if let Ok(attr) = e.try_get_attribute("term") {
                            paper.categories.push(attr.map(|a| a.unescape_value().map(|v| v.into_owned()).unwrap_or_default()).unwrap_or_default());
                        }
                    }
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
                        // http://arxiv.org/abs/2607.12631v1 → strip version suffix for the base ID
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
                        if let Some(paper) = current.take() {
                            if !paper.arxiv_id.is_empty() {
                                papers.push(paper);
                            }
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
    // Collapse whitespace, arXiv abstracts contain newlines.
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "live arXiv test — run with --ignored to avoid rate limits"]
    async fn live_search() {
        // arXiv rate-limits bursty test traffic (429); retry with backoff.
        let a = Arxiv::new();
        let mut last_err: Option<String> = None;
        for attempt in 0..3 {
            match a.search("induced emotion LLM decision making", 3).await {
                Ok(papers) if !papers.is_empty() => {
                    for p in papers.iter().take(3) {
                        assert!(!p.arxiv_id.is_empty());
                        assert!(!p.title.is_empty());
                        assert!(p.url.starts_with("https://arxiv.org/abs/"));
                        println!("  [{}] {}", p.arxiv_id, p.title);
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

    #[tokio::test]
    #[ignore = "live arXiv test — run with --ignored to avoid rate limits"]
    async fn live_fetch_by_id() {
        let a = Arxiv::new();
        let mut last_err: Option<String> = None;
        for attempt in 0..3 {
            match a.by_id("2607.12631").await {
                Ok(Some(paper)) => {
                    assert_eq!(paper.arxiv_id, "2607.12631");
                    assert!(!paper.abstract_text.is_empty(), "abstract should be present");
                    println!("  title: {}", paper.title);
                    println!("  authors: {}", paper.authors.join(", "));
                    return;
                }
                Ok(None) => last_err = Some("not found".into()),
                Err(e) => last_err = Some(e.to_string()),
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
        panic!("arxiv fetch failed after retries: {}", last_err.unwrap_or_default());
    }
}
