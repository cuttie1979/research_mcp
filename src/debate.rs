//! Multi-agent debate engine — adapted from MiroShark's belief state pattern.
//!
//! N agents with distinct initial beliefs debate a cited draft. Each round:
//! 1. Every agent writes a short argument from its current belief (content +
//!    reference critique).
//! 2. Every agent reads the others' arguments and updates its belief
//!    (LLM-based: strong arguments shift positions, weak ones don't).
//! After N rounds the engine measures consensus (positions, spread,
//! convergence) and produces a debate summary for the review phase.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::llm::{ChatMessage, Llm};
use crate::workflow::EvidenceItem;

pub const DEFAULT_ROUNDS: usize = 2;
pub const DEFAULT_AGENT_COUNT: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateAgent {
    pub id: String,
    pub name: String,
    /// Stance label injected into the persona.
    pub stance: String,
    /// Current position: -1.0 (strongly against) to +1.0 (strongly for).
    pub position: f32,
    /// Certainty 0.0–1.0. High confidence resists change.
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateArgument {
    pub agent: String,
    pub round: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateResult {
    pub agents: Vec<DebateAgent>,
    pub rounds: Vec<Vec<DebateArgument>>,
    pub consensus: ConsensusSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusSummary {
    /// Per-agent final positions.
    pub final_positions: Vec<AgentPosition>,
    /// Mean position across agents (-1..1).
    pub mean_position: f32,
    /// Spread = max - min. Lower = more consensus.
    pub spread: f32,
    /// first_round_spread - last_round_spread. Positive = converged.
    pub convergence: f32,
    /// Consensus points (agents agree).
    pub consensus_points: Vec<String>,
    /// Dissensus points (agents disagree).
    pub dissensus_points: Vec<String>,
    /// Verdict text for the review phase.
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPosition {
    pub agent: String,
    pub position: f32,
    pub confidence: f32,
}

/// Initial agent lineup — distinct beliefs across the epistemic spectrum.
fn initial_agents() -> Vec<DebateAgent> {
    vec![
        DebateAgent {
            id: "A1".into(),
            name: "The Skeptic".into(),
            stance: "skeptical, demands strong evidence for every claim".into(),
            position: -0.6,
            confidence: 0.6,
        },
        DebateAgent {
            id: "A2".into(),
            name: "The Advocate".into(),
            stance: "charitable to the draft's thesis, looks for what's right".into(),
            position: 0.6,
            confidence: 0.5,
        },
        DebateAgent {
            id: "A3".into(),
            name: "The Methodologist".into(),
            stance: "focuses on methodology, source quality, and citation rigor".into(),
            position: 0.1,
            confidence: 0.7,
        },
        DebateAgent {
            id: "A4".into(),
            name: "The Practitioner".into(),
            stance: "grounds every claim in practical applicability and real-world context".into(),
            position: 0.2,
            confidence: 0.5,
        },
        DebateAgent {
            id: "A5".into(),
            name: "The Ethicist".into(),
            stance: "weighs risks, biases, and the broader implications of the conclusions".into(),
            position: -0.2,
            confidence: 0.5,
        },
    ]
}

/// Run the debate on a cited draft. Returns the full debate result.
pub async fn run_debate(
    llm: &Llm,
    topic: &str,
    cited_draft: &str,
    evidence: &[EvidenceItem],
    temperature: f32,
    rounds: usize,
) -> Result<DebateResult> {
    let mut agents = initial_agents();
    let mut round_arguments: Vec<Vec<DebateArgument>> = Vec::new();
    let mut first_round_spread: Option<f32> = None;
    let mut last_round_spread: f32 = 0.0;

    let evidence_block = build_source_list(evidence);

    // Context economy: agents get a condensed draft (exec summary + open
    // questions + sources), not the full text — keeps LLM calls fast.
    let draft_context = condense_draft(cited_draft);

    for round in 1..=rounds {
        log_info!("  ── debate round {round}/{} ──", rounds);
        let mut this_round: Vec<DebateArgument> = Vec::new();

        // Step 1: each agent argues from its current belief.
        for agent in &agents {
            let arg = agent_argument(llm, agent, topic, &draft_context, &evidence_block, temperature).await?;
            this_round.push(DebateArgument {
                agent: agent.id.clone(),
                round,
                text: arg,
            });
        }

        // Step 2: each agent reads the others' arguments and updates belief.
        for agent in &mut agents {
            let others: Vec<&DebateArgument> = this_round
                .iter()
                .filter(|a| a.agent != agent.id)
                .collect();
            let (new_pos, new_conf) = agent_update(llm, agent, &others, temperature).await?;
            agent.position = new_pos;
            agent.confidence = new_conf;
        }

        // Measure spread for this round.
        let positions: Vec<f32> = agents.iter().map(|a| a.position).collect();
        let spread = max_f(&positions) - min_f(&positions);
        last_round_spread = spread;
        if first_round_spread.is_none() {
            first_round_spread = Some(spread);
        }
        log_info!("  spread after round {round}: {spread:.2}");

        round_arguments.push(this_round);
    }

    // Consensus measurement.
    let final_positions: Vec<AgentPosition> = agents
        .iter()
        .map(|a| AgentPosition {
            agent: a.name.clone(),
            position: a.position,
            confidence: a.confidence,
        })
        .collect();
    let mean_position = final_positions.iter().map(|p| p.position).sum::<f32>() / final_positions.len() as f32;
    let spread = max_f(&final_positions.iter().map(|p| p.position).collect::<Vec<_>>())
        - min_f(&final_positions.iter().map(|p| p.position).collect::<Vec<_>>());
    let convergence = first_round_spread.unwrap_or(0.0) - last_round_spread;

    let (consensus_points, dissensus_points) =
        summarize_agreement(llm, topic, &final_positions, &round_arguments, temperature).await?;

    let summary = build_summary(&final_positions, mean_position, spread, convergence);

    Ok(DebateResult {
        agents,
        rounds: round_arguments,
        consensus: ConsensusSummary {
            final_positions,
            mean_position,
            spread,
            convergence,
            consensus_points,
            dissensus_points,
            summary,
        },
    })
}

/// Condense a cited draft for debate: Executive Summary + Open Questions +
/// the Sources list. Falls back to the first 2000 chars if sections are absent.
pub fn condense_draft(cited_draft: &str) -> String {
    let mut out = String::new();

    if let Some(pos) = cited_draft.find("## Executive Summary") {
        let end = cited_draft[pos..]
            .find("\n## ")
            .map(|i| pos + i)
            .unwrap_or(cited_draft.len());
        out.push_str(&cited_draft[pos..end]);
        out.push_str("\n\n");
    }

    if let Some(pos) = cited_draft.find("## Open Questions") {
        let end = cited_draft[pos..]
            .find("\n## ")
            .map(|i| pos + i)
            .unwrap_or(cited_draft.len());
        out.push_str(&cited_draft[pos..end]);
        out.push_str("\n\n");
    }

    // Sources section (titles + URLs only, strip long content).
    if let Some(pos) = cited_draft.find("## Sources") {
        out.push_str(&cited_draft[pos..]);
    }

    if out.trim().is_empty() {
        out.push_str(&cited_draft.chars().take(2000).collect::<String>());
    }

    // Hard cap to keep the prompt bounded.
    out.chars().take(6000).collect()
}

/// Agent writes an argument from its current belief.
async fn agent_argument(
    llm: &Llm,
    agent: &DebateAgent,
    topic: &str,
    cited_draft: &str,
    source_list: &str,
    temperature: f32,
) -> Result<String> {
    let sys = ChatMessage::system(
        "You are a research debate participant. You are critiquing a research brief \
         before it is finalized. You examine BOTH the content claims AND the references \
         that support them.\n\
         Rules:\n\
         - Argue from your stated stance and current belief. Do not abandon your position lightly.\n\
         - Focus on: unsupported claims, weak or missing citations, single-source critical claims,\n\
           logical gaps, overstatement, and reference quality.\n\
         - Be specific: name the claim or citation you challenge, and say what evidence would change your mind.\n\
         - Keep it under 150 words. No pleasantries.\n\
         - Output only your argument.",
    );
    let belief = format!(
        "# YOUR CURRENT BELIEFS AND STANCE\n- Stance: {}\n- Position on the topic: {:.2} (-1 strongly against, +1 strongly for)\n- Confidence: {:.2}",
        agent.stance, agent.position, agent.confidence
    );
    let user = ChatMessage::user(format!(
        "Topic: {topic}\n\nSources available:\n{source_list}\n\nCited draft under debate:\n\n{cited_draft}\n\n{belief}\n\nWrite your argument."
    ));
    llm.complete(&[sys, user], temperature).await
}

/// Agent reads others' arguments and updates position + confidence.
async fn agent_update(
    llm: &Llm,
    agent: &DebateAgent,
    others: &[&DebateArgument],
    temperature: f32,
) -> Result<(f32, f32)> {
    let sys = ChatMessage::system(
        "You are a research debate participant. You have just read your colleagues' arguments.\n\
         Update your belief:\n\
         - If an argument is strong (specific, evidence-backed, addresses your concerns), move toward it.\n\
         - If an argument is weak (vague, unsupported, already refuted), ignore it.\n\
         - High-confidence positions resist change; low-confidence ones are more movable.\n\
         - Your position stays within -1.0 (strongly against) to +1.0 (strongly for).\n\
         Respond with a single JSON object:\n\
         {\"position\": <number -1..1>, \"confidence\": <number 0..1>, \"reason\": \"<one sentence>\"}\n\
         No markdown, only the JSON object.",
    );
    let others_text: Vec<String> = others
        .iter()
        .map(|a| format!("[{}]: {}", a.agent, a.text))
        .collect();
    let user = ChatMessage::user(format!(
        "Your current belief: position {:.2}, confidence {:.2}\n\nArguments you read:\n{}\n\nOutput your updated belief JSON.",
        agent.position,
        agent.confidence,
        others_text.join("\n\n")
    ));
    let raw = llm.complete_json(&[sys, user], temperature).await?;
    let parsed: serde_json::Value = serde_json::from_str(raw.trim())
        .or_else(|_| {
            // Strip code fences if wrapped.
            let t = raw.trim();
            let json = t
                .strip_prefix("```json")
                .or_else(|| t.strip_prefix("```"))
                .and_then(|s| s.strip_suffix("```"))
                .unwrap_or(t);
            serde_json::from_str(json.trim())
        })
        .context("parse belief update JSON")?;
    let position = parsed.get("position").and_then(|v| v.as_f64()).unwrap_or(agent.position as f64) as f32;
    let confidence = parsed.get("confidence").and_then(|v| v.as_f64()).unwrap_or(agent.confidence as f64) as f32;
    Ok((
        position.clamp(-1.0, 1.0),
        confidence.clamp(0.0, 1.0),
    ))
}

/// LLM summarizes agreement: consensus points + dissensus points.
async fn summarize_agreement(
    llm: &Llm,
    topic: &str,
    final_positions: &[AgentPosition],
    round_arguments: &[Vec<DebateArgument>],
    temperature: f32,
) -> Result<(Vec<String>, Vec<String>)> {
    let sys = ChatMessage::system(
        "You are a debate moderator. Based on the final agent positions and the full debate transcript,\n\
         identify:\n\
         1. consensus_points: claims or issues where the agents converged (agreement). 2-4 items.\n\
         2. dissensus_points: claims or issues where agents still disagree. 2-4 items.\n\
         Respond with a single JSON object:\n\
         {\"consensus_points\": [\"...\"], \"dissensus_points\": [\"...\"]}\n\
         No markdown, only the JSON object.",
    );
    let positions_text: Vec<String> = final_positions
        .iter()
        .map(|p| format!("{}: position {:.2}, confidence {:.2}", p.agent, p.position, p.confidence))
        .collect();
    let transcript: Vec<String> = round_arguments
        .iter()
        .flatten()
        .map(|a| format!("[R{} {}]: {}", a.round, a.agent, a.text))
        .collect();
    let user = ChatMessage::user(format!(
        "Topic: {topic}\n\nFinal positions:\n{}\n\nDebate transcript:\n{}\n\nOutput the consensus JSON.",
        positions_text.join("\n"),
        transcript.join("\n")
    ));
    let raw = llm.complete_json(&[sys, user], temperature).await?;
    let v: serde_json::Value = serde_json::from_str(raw.trim())
        .or_else(|_| {
            let t = raw.trim();
            let json = t
                .strip_prefix("```json")
                .or_else(|| t.strip_prefix("```"))
                .and_then(|s| s.strip_suffix("```"))
                .unwrap_or(t);
            serde_json::from_str(json.trim())
        })
        .context("parse consensus JSON")?;
    let consensus = v
        .get("consensus_points")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    let dissensus = v
        .get("dissensus_points")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    Ok((consensus, dissensus))
}

fn build_summary(
    positions: &[AgentPosition],
    mean: f32,
    spread: f32,
    convergence: f32,
) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "Debate concluded: mean position {mean:.2}, spread {spread:.2}, convergence {convergence:+.2} "
    ));
    s.push_str(&format!("({} of {} agents are within 0.3 of the mean).\n", positions.iter().filter(|p| (p.position - mean).abs() < 0.3).count(), positions.len()));
    s.push_str("Final positions:\n");
    for p in positions {
        s.push_str(&format!("  - {}: {:.2} (conf {:.2})\n", p.agent, p.position, p.confidence));
    }
    s
}

fn build_source_list(evidence: &[EvidenceItem]) -> String {
    let mut s = String::new();
    for e in evidence {
        s.push_str(&format!("[{0}] {1} — {2}\n", e.id, e.title, e.url));
    }
    s
}

fn min_f(v: &[f32]) -> f32 {
    v.iter().cloned().fold(f32::MAX, f32::min)
}

fn max_f(v: &[f32]) -> f32 {
    v.iter().cloned().fold(f32::MIN, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_agents_cover_spectrum() {
        let agents = initial_agents();
        assert_eq!(agents.len(), 5);
        let positions: Vec<f32> = agents.iter().map(|a| a.position).collect();
        let min = min_f(&positions);
        let max = max_f(&positions);
        assert!(min < 0.0, "at least one negative position, got {min}");
        assert!(max > 0.0, "at least one positive position, got {max}");
        // All five have distinct stances.
        let names: std::collections::HashSet<&str> = agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names.len(), 5);
    }

    #[test]
    fn min_max_helpers() {
        assert_eq!(min_f(&[3.0, -1.0, 0.5]), -1.0);
        assert_eq!(max_f(&[3.0, -1.0, 0.5]), 3.0);
    }

    #[test]
    fn result_serializes_roundtrip() {
        let agents = initial_agents();
        let result = DebateResult {
            agents: agents.clone(),
            rounds: vec![],
            consensus: ConsensusSummary {
                final_positions: agents
                    .iter()
                    .map(|a| AgentPosition { agent: a.name.clone(), position: a.position, confidence: a.confidence })
                    .collect(),
                mean_position: 0.0,
                spread: 1.2,
                convergence: 0.3,
                consensus_points: vec!["point".into()],
                dissensus_points: vec![],
                summary: "summary".into(),
            },
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: DebateResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.agents.len(), 5);
        assert_eq!(back.consensus.spread, 1.2);
        assert_eq!(back.consensus.consensus_points, vec!["point".to_string()]);
    }

    #[test]
    fn build_summary_reports_spread() {
        let positions = vec![
            AgentPosition { agent: "a".into(), position: -0.5, confidence: 0.6 },
            AgentPosition { agent: "b".into(), position: 0.5, confidence: 0.6 },
        ];
        let s = build_summary(&positions, 0.0, 1.0, 0.2);
        assert!(s.contains("mean position 0.00"));
        assert!(s.contains("spread 1.00"));
        assert!(s.contains("convergence +0.20"));
    }
}

#[cfg(test)]
mod helpers_tests {
    use super::*;

    #[test]
    fn condense_extracts_sections() {
        let draft = "# Title\n\n## Executive Summary\n\nKey findings here.\n\n## Findings\n\nLong content.\n\n## Open Questions\n\nQ1 remains.\n\n## Sources\n\n[S1] url";
        let c = condense_draft(draft);
        assert!(c.contains("## Executive Summary"));
        assert!(c.contains("Key findings here."));
        assert!(c.contains("## Open Questions"));
        assert!(c.contains("Q1 remains."));
        assert!(c.contains("## Sources"));
        assert!(!c.contains("Long content."), "findings section should be stripped");
    }

    #[test]
    fn condense_falls_back_to_head() {
        let draft = "No structured sections here. Just text. ".repeat(200);
        let c = condense_draft(&draft);
        assert!(c.len() <= 6000);
        assert!(!c.is_empty());
    }
}
