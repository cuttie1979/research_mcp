//! Scopus API client (Elsevier).
//!
//! Default API key access (no special approval needed):
//! - Scopus Search API — metadata: title, creators, DOI, EID, journal, year, citedby
//! - Abstract Retrieval META view — basic metadata
//!
//! Abstract full text requires the META_ABS view, which needs Elsevier
//! approval. As a fallback we query CrossRef by DOI (free, no key) for
//! abstracts that publishers deposit. The DOI link itself is also fetchable.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

pub struct Scopus {
    client: reqwest::Client,
    api_key: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScopusPaper {
    pub eid: String,
    pub scopus_id: String,
    pub doi: Option<String>,
    pub title: String,
    pub creators: Vec<String>,
    pub journal: Option<String>,
    pub year: Option<String>,
    pub citedby: i64,
    pub url: String,
    /// Abstract fetched from CrossRef by DOI (may be empty).
    pub abstract_text: String,
}

impl Scopus {
    pub fn new(api_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
        }
    }

    /// Search Scopus by query. Returns up to `max` papers with metadata.
    pub async fn search(&self, query: &str, max: usize) -> Result<Vec<ScopusPaper>> {
        let resp = self
            .client
            .get("https://api.elsevier.com/content/search/scopus")
            .query(&[("query", query), ("count", &max.to_string())])
            .header("X-ELS-APIKey", &self.api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .context("Scopus search request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Scopus API error {}: {}", status, &body[..body.len().min(200)]);
        }
        let data: ScopusSearchResponse = resp.json().await.context("Scopus search parse failed")?;
        Ok(data.into_papers())
    }

    /// Fetch abstract text for a DOI via CrossRef (free, no key).
    pub async fn abstract_by_doi(&self, doi: &str) -> Result<String> {
        let url = format!("https://api.crossref.org/works/{doi}");
        let resp = self
            .client
            .get(&url)
            .header("User-Agent", "research_mcp/0.1 (mailto:research@local.invalid)")
            .send()
            .await
            .context("CrossRef request failed")?;
        if !resp.status().is_success() {
            return Ok(String::new());
        }
        let data: CrossRefResponse = resp.json().await.context("CrossRef parse failed")?;
        let abs = data
            .message
            .abstract_text
            .unwrap_or_default()
            .replace("<jats:p>", " ")
            .replace("</jats:p>", "\n")
            .replace("<jats:title>", " ")
            .replace("</jats:title>", "\n")
            .replace("<jats:sec>", "")
            .replace("</jats:sec>", "");
        Ok(abs.trim().to_string())
    }
}

#[derive(Debug, Deserialize)]
struct ScopusSearchResponse {
    #[serde(rename = "search-results")]
    search_results: Option<SearchResults>,
}

#[derive(Debug, Deserialize)]
struct SearchResults {
    #[serde(default)]
    entry: Vec<ScopusEntry>,
}

#[derive(Debug, Deserialize)]
struct ScopusEntry {
    #[serde(rename = "dc:title", default)]
    title: String,
    #[serde(rename = "dc:creator", default)]
    creator: String,
    #[serde(rename = "prism:doi", default)]
    doi: Option<String>,
    #[serde(default)]
    eid: String,
    #[serde(rename = "dc:identifier", default)]
    dc_identifier: String,
    #[serde(rename = "prism:publicationName", default)]
    journal: Option<String>,
    #[serde(rename = "prism:coverDate", default)]
    cover_date: Option<String>,
    #[serde(rename = "citedby-count", default)]
    citedby_count: Option<String>,
}

impl ScopusSearchResponse {
    fn into_papers(self) -> Vec<ScopusPaper> {
        let entries = self
            .search_results
            .map(|r| r.entry)
            .unwrap_or_default();
        entries
            .into_iter()
            .filter(|e| !e.title.is_empty() || e.doi.is_some())
            .map(|e| {
                let scopus_id = e
                    .dc_identifier
                    .trim_start_matches("SCOPUS_ID:")
                    .to_string();
                let year = e.cover_date.as_deref().map(|d| d.split('-').next().unwrap_or(d).to_string());
                let citedby = e
                    .citedby_count
                    .as_deref()
                    .and_then(|c| c.parse::<i64>().ok())
                    .unwrap_or(0);
                ScopusPaper {
                    eid: e.eid.clone(),
                    scopus_id,
                    doi: e.doi.clone(),
                    title: clean(&e.title),
                    creators: e.creator.split(';').map(|c| clean(c)).filter(|c| !c.is_empty()).collect(),
                    journal: e.journal.map(|j| clean(&j)),
                    year,
                    citedby,
                    url: e
                        .doi
                        .map(|d| format!("https://doi.org/{d}"))
                        .unwrap_or_else(|| format!("https://www.scopus.com/inward/record.uri?scp={}", e.eid)),
                    abstract_text: String::new(),
                }
            })
            .collect()
    }
}

#[derive(Debug, Deserialize)]
struct CrossRefResponse {
    message: CrossRefMessage,
}

#[derive(Debug, Deserialize)]
struct CrossRefMessage {
    #[serde(rename = "abstract", default)]
    abstract_text: Option<String>,
}

fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires ELSEVIER_API_KEY — run with --ignored"]
    async fn live_search() {
        let key = std::env::var("ELSEVIER_API_KEY").expect("ELSEVIER_API_KEY env");
        let s = Scopus::new(key);
        let papers = s.search("TITLE(LLM decision making)", 3).await.expect("scopus search works");
        assert!(!papers.is_empty(), "expected results");
        for p in papers.iter().take(3) {
            assert!(!p.title.is_empty());
            println!("  [{}] {}", p.eid, p.title);
        }
    }

    #[tokio::test]
    #[ignore = "requires ELSEVIER_API_KEY — run with --ignored"]
    async fn live_crossref_abstract() {
        let s = Scopus::new("unused".to_string());
        let abs = s.abstract_by_doi("10.1007/s13755-026-00449-8").await.unwrap_or_default();
        // Abstract may be empty if the publisher doesn't deposit it.
        println!("abstract len: {}", abs.len());
    }
}
