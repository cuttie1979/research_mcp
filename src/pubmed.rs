//! PubMed API client — NCBI E-utilities (esearch + efetch), no API key required.
//! Endpoints:
//!   esearch: https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi
//!   efetch:  https://eutils.ncbi.nlm.nih.gov/entrez/eutils/efetch.fcgi

use anyhow::{bail, Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Deserialize;

const ESEARCH_URL: &str = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi";
const EFETCH_URL: &str = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/efetch.fcgi";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PubmedPaper {
    pub pmid: String,
    pub title: String,
    pub authors: Vec<String>,
    pub abstract_text: String,
    pub url: String,
    pub journal: Option<String>,
    pub published: Option<String>,
    pub doi: Option<String>,
}

pub struct Pubmed {
    client: reqwest::Client,
}

impl Pubmed {
    pub fn new() -> Self {
        Self { client: reqwest::Client::new() }
    }

    /// Search PubMed by query, sorted by relevance. Returns full records.
    pub async fn search(&self, query: &str, max_results: usize) -> Result<Vec<PubmedPaper>> {
        // Step 1: esearch → PMIDs.
        let ids = self.esearch(query, max_results).await?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // Step 2: efetch → full records.
        let papers = self.efetch(&ids).await?;
        Ok(papers)
    }

    /// Fetch a single paper by PMID.
    pub async fn by_id(&self, pmid: &str) -> Result<Option<PubmedPaper>> {
        let ids = vec![pmid.to_string()];
        let papers = self.efetch(&ids).await?;
        Ok(papers.into_iter().next())
    }

    async fn esearch(&self, query: &str, max_results: usize) -> Result<Vec<String>> {
        let resp = self
            .client
            .get(ESEARCH_URL)
            .query(&[
                ("db", "pubmed"),
                ("term", query),
                ("retmax", &max_results.to_string()),
                ("retmode", "json"),
                ("sort", "relevance"),
                ("tool", "research_mcp"),
                ("email", "research@local.invalid"),
            ])
            .header("User-Agent", "research_mcp/0.1 (Rust deepresearch tool)")
            .send()
            .await
            .context("PubMed esearch request failed")?;

        if !resp.status().is_success() {
            bail!("PubMed esearch error {}", resp.status());
        }
        let parsed: ESearchResponse = resp.json().await.context("PubMed esearch parse failed")?;
        Ok(parsed.esearchresult.idlist)
    }

    async fn efetch(&self, pmids: &[String]) -> Result<Vec<PubmedPaper>> {
        if pmids.is_empty() {
            return Ok(Vec::new());
        }
        let resp = self
            .client
            .get(EFETCH_URL)
            .query(&[
                ("db", "pubmed"),
                ("id", &pmids.join(",")),
                ("retmode", "xml"),
                ("tool", "research_mcp"),
                ("email", "research@local.invalid"),
            ])
            .header("User-Agent", "research_mcp/0.1 (Rust deepresearch tool)")
            .send()
            .await
            .context("PubMed efetch request failed")?;

        if !resp.status().is_success() {
            bail!("PubMed efetch error {}", resp.status());
        }
        let body = resp.text().await.context("PubMed efetch read failed")?;
        parse_records(&body)
    }
}

#[derive(Debug, Deserialize)]
struct ESearchResponse {
    esearchresult: ESearchResult,
}

#[derive(Debug, Deserialize)]
struct ESearchResult {
    #[serde(default)]
    idlist: Vec<String>,
}

/// Parse PubMed efetch XML (PubmedArticleSet) into papers.
fn parse_records(xml: &str) -> Result<Vec<PubmedPaper>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut papers: Vec<PubmedPaper> = Vec::new();
    let mut current: Option<PubmedPaper> = None;
    let mut current_tag = String::new();
    let mut in_author = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                current_tag = name.clone();
                match name.as_str() {
                    "PubmedArticle" => {
                        current = Some(PubmedPaper {
                            pmid: String::new(),
                            title: String::new(),
                            authors: Vec::new(),
                            abstract_text: String::new(),
                            url: String::new(),
                            journal: None,
                            published: None,
                            doi: None,
                        });
                    }
                    "Author" => in_author = true,
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                if current.is_none() {
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
                    "PMID" => {
                        paper.pmid = text.clone();
                        paper.url = format!("https://pubmed.ncbi.nlm.nih.gov/{text}/");
                    }
                    "ArticleTitle" => paper.title = clean_text(&text),
                    "AbstractText" => {
                        paper.abstract_text.push_str(&text);
                        paper.abstract_text.push(' ');
                    }
                    "Title" if !paper.journal.is_some() => paper.journal = Some(text),
                    "LastName" if in_author => {
                        paper.authors.push(text.clone());
                    }
                    "ForeName" if in_author => {
                        if let Some(last) = paper.authors.last_mut() {
                            *last = format!("{text} {last}");
                        }
                    }
                    "Year" if paper.published.is_none() => paper.published = Some(text),
                    "ELocationID" if paper.doi.is_none() => paper.doi = Some(text),
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                match name.as_str() {
                    "PubmedArticle" => {
                        if let Some(mut paper) = current.take() {
                            paper.abstract_text = clean_text(&paper.abstract_text);
                            if !paper.pmid.is_empty() {
                                papers.push(paper);
                            }
                        }
                    }
                    "Author" => in_author = false,
                    _ => current_tag.clear(),
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow::anyhow!("PubMed XML parse error: {e}")),
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
    async fn live_search() {
        let p = Pubmed::new();
        let papers = p.search("LLM decision making bias emotion", 3).await.expect("pubmed search works");
        assert!(!papers.is_empty(), "expected results");
        for pp in papers.iter().take(3) {
            assert!(!pp.pmid.is_empty());
            assert!(!pp.title.is_empty());
            assert!(pp.url.starts_with("https://pubmed.ncbi.nlm.nih.gov/"));
            println!("  [{}] {}", pp.pmid, pp.title);
        }
    }

    #[tokio::test]
    async fn live_fetch_by_id() {
        let p = Pubmed::new();
        // Classic IGT paper.
        let paper = p.by_id("11504948").await.expect("fetch by id works").expect("paper exists");
        assert_eq!(paper.pmid, "11504948");
        assert!(!paper.abstract_text.is_empty(), "abstract should be present");
        println!("  title: {}", paper.title);
        println!("  authors: {}", paper.authors.join(", "));
    }
}
