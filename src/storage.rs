//! Local SQLite storage for staged learning and semantic-map claims.

use std::fs;
use std::io;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::knowledge::{Claim, KnowledgeMap};

const SCHEMA_VERSION: i64 = 42;

#[derive(Debug)]
pub struct ScholarFeedbackTask {
    pub id: i64,
    pub finding_id: i64,
    pub root_finding_id: i64,
    pub attempt: i64,
    pub guidance: String,
    pub job_id: String,
    pub question: String,
    pub source_title: String,
    pub source_url: String,
    pub source_excerpt: String,
    pub source_quality: String,
    pub source_updated_at: Option<String>,
    pub rejected_claim: String,
    pub suggested_evidence: Option<String>,
    pub exclusion_reason: Option<String>,
}
const PROMOTION_CONFIDENCE: f64 = 0.95;
const DEFAULT_AUTO_PROMOTION_CONFIDENCE: f64 = 0.90;
const ASSISTANT_BASE_CONFIDENCE: f64 = 0.5;
const SEVERE_CHALLENGE_MULTIPLIER: f64 = 0.2;

pub(crate) fn scout_min_interval_seconds() -> u64 {
    std::env::var("COMMUNITY_SCOUT_MIN_INTERVAL_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value <= 86_400)
        .unwrap_or(300)
}

pub(crate) fn automatic_promotion_confidence() -> f64 {
    parse_automatic_promotion_confidence(
        std::env::var("SCHOLAR_AUTO_PROMOTION_CONFIDENCE")
            .ok()
            .as_deref(),
    )
}

fn parse_automatic_promotion_confidence(value: Option<&str>) -> f64 {
    value
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
        .unwrap_or(DEFAULT_AUTO_PROMOTION_CONFIDENCE)
}

pub struct LearningStore {
    connection: Connection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub id: String,
    pub conversation_id: String,
    pub turn_number: usize,
    pub raw_text: String,
    pub status: String,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CandidateClaim {
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub evidence_text: String,
    pub context_message_ids: Vec<String>,
    pub resolution: Option<String>,
    pub confidence: f64,
    pub status: String,
    pub decision_reason: Option<String>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticClaim {
    pub id: i64,
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub source: String,
    pub source_observation_id: Option<String>,
    pub status: String,
    pub volatility: String,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub observed_at: Option<String>,
    pub checked_at: Option<String>,
    pub last_reviewed_at: Option<String>,
    pub next_review_at: Option<String>,
    pub extraction_confidence: Option<f64>,
    pub source_quality: Option<String>,
    pub freshness: Option<String>,
    /// Relevance of the Scholar finding to the question that sourced it.
    /// This affects retrieval priority, not factual confidence.
    pub research_relevance: Option<String>,
}

impl Default for SemanticClaim {
    fn default() -> Self {
        Self {
            id: 0,
            subject: String::new(),
            relation: String::new(),
            object: String::new(),
            source: String::new(),
            source_observation_id: None,
            status: "active".into(),
            volatility: "stable".into(),
            valid_from: None,
            valid_until: None,
            observed_at: None,
            checked_at: None,
            last_reviewed_at: None,
            next_review_at: None,
            extraction_confidence: None,
            source_quality: None,
            freshness: None,
            research_relevance: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxSummary {
    pub pending: usize,
    pub processing: usize,
    pub needs_review: usize,
    pub observations: Vec<Observation>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredCandidate {
    pub id: String,
    pub observation_id: String,
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub evidence_text: String,
    pub confidence: f64,
    pub status: String,
    pub decision_reason: Option<String>,
    pub semantic_claim_id: Option<i64>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObservationDetail {
    pub observation: Observation,
    pub last_error: Option<String>,
    pub candidates: Vec<StoredCandidate>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticClaimDetail {
    pub claim: SemanticClaim,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimEvidence {
    pub id: i64,
    pub source_kind: String,
    pub source_uri: Option<String>,
    pub observation_id: Option<String>,
    pub excerpt: String,
    pub stance: String,
    pub published_at: Option<String>,
    pub retrieved_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScholarJob {
    pub id: String,
    pub question: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorroborationTask {
    pub finding_id: i64,
    pub provider: String,
    pub job_id: String,
    pub question: String,
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorroborationQueueItem {
    pub finding_id: i64,
    pub claim: String,
    pub status: String,
    pub serp_attempts: i64,
    pub tavily_attempts: i64,
    pub claimed_provider: Option<String>,
    pub last_outcome: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoutInvestigation {
    pub trigger_kind: String,
    pub topic_key: String,
    pub trigger_ref: String,
    pub question: String,
    pub scholar_job_id: String,
    pub selection_reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoutModelProbe {
    pub id: i64,
    pub status: String,
    pub seed: String,
    pub topic: Option<String>,
    pub question: Option<String>,
    pub selection_reason: String,
    pub scholar_job_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScholarSearchAttempt {
    pub provider: String,
    pub trigger_reason: String,
    pub query: String,
    pub status: String,
    pub result_count: Option<usize>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostRequestReservation {
    pub id: i64,
    pub scheduled_at_ms: u128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TavilyReservation {
    pub id: i64,
    pub credits: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerpApiReservation {
    pub id: i64,
    pub scheduled_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerpApiDomainObservation {
    pub searched_at_ms: i64,
    pub query: String,
    pub domain: String,
    pub result_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScholarSearchCandidate {
    pub id: i64,
    pub provider: String,
    pub query: String,
    pub result_type: String,
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub structured_data_json: String,
    pub status: String,
    pub policy_status: String,
    pub policy_reason: Option<String>,
    pub fetch_status: String,
    pub fetched_url: Option<String>,
    pub fetched_excerpt: Option<String>,
    pub fetched_at: Option<String>,
    /// Declared HTML metadata; useful for review, never proof of independence.
    pub page_provenance_json: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScholarProvenanceReview {
    pub id: i64,
    pub finding_id: i64,
    pub related_finding_id: i64,
    pub signal: String,
    pub source_url: String,
    pub related_source_url: String,
    pub source_excerpt: String,
    pub related_source_excerpt: String,
    pub source_metadata_json: Option<String>,
    pub related_metadata_json: Option<String>,
    pub status: String,
    pub verdict: Option<String>,
    pub rationale: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScholarRobotsBlock {
    pub host: String,
    pub display_path: String,
    pub reason: String,
    pub expires_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScholarExtractionAttempt {
    pub attempt_index: usize,
    pub passage: String,
    pub status: String,
    pub reason: String,
    pub model_output: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MaintenanceRoleRun {
    pub role: String,
    pub status: String,
    pub processed: usize,
    pub detail: Option<String>,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MaintenancePass {
    pub id: i64,
    pub mode: String,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub duration_ms: Option<u64>,
    pub roles: Vec<MaintenanceRoleRun>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MaintenanceQueue {
    pub pending: usize,
    pub oldest_age_seconds: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MaintenanceStatus {
    pub last_pass: Option<MaintenancePass>,
    pub curator: MaintenanceQueue,
    pub scout: MaintenanceQueue,
    pub scholar: MaintenanceQueue,
    pub scan_cursor: i64,
    pub scout_priority_order: Vec<&'static str>,
    pub latest_scout_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaintenanceDecision {
    pub id: i64,
    pub role: String,
    pub source_type: String,
    pub source_id: String,
    pub suggested_action: String,
    pub outcome: String,
    pub reason: String,
    pub resulting_claim_id: Option<i64>,
    pub resulting_state: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaintenanceReviewClaim {
    pub id: i64,
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub relation_count: usize,
    pub evidence_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenRoleProposal {
    pub role: String,
    pub action: String,
    pub outcome: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaintenanceWorkCounts {
    pub curator_reviews: usize,
    pub scholar_reviews: usize,
    pub pending_research: usize,
    pub challenged_claims: usize,
    pub due_claims: usize,
}

pub struct ModelConsolidation {
    pub id: i64,
    pub claim_id: i64,
    pub source_checkpoint: String,
    pub output_checkpoint: String,
    pub training_loss: f32,
    pub manifest_path: Option<String>,
    pub base_checkpoint_sha256: Option<String>,
    pub included_claim_ids: Vec<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConsolidatedClaimRevision {
    pub id: i64,
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub source: String,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ModelCheckpointManifest {
    pub version: u32,
    pub base_model_dir: String,
    pub base_checkpoint: String,
    pub base_checkpoint_sha256: String,
    pub base_adapter_snapshot: Option<String>,
    pub base_adapter_sha256: Option<String>,
    pub included_claims: Vec<ConsolidatedClaimRevision>,
    pub system_prompt: String,
    pub max_training_passes: usize,
    pub loss_threshold: f32,
    pub probe_max_chars: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScholarFinding {
    pub semantic_support: Option<crate::semantic::SemanticSupport>,
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub evidence_text: String,
    pub confidence: f64,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub source_quality: String,
    pub source_updated_at: Option<String>,
}

/// Source facts established by an adapter or reviewer, never inferred from a
/// search snippet or a different hostname alone.
#[derive(Clone, Debug, Default)]
pub struct ScholarSourceProfile {
    pub published_at: Option<String>,
    pub verified_underlying_source_id: Option<String>,
    pub topical_expertise_verified: bool,
}

#[derive(Clone, Debug)]
pub struct ScholarEvidence {
    pub source_title: String,
    pub source_url: String,
    pub source_excerpt: String,
    pub finding: Option<ScholarFinding>,
    pub profile: ScholarSourceProfile,
    pub research_relevance: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredScholarFinding {
    pub id: i64,
    pub source_url: String,
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub evidence_text: String,
    pub confidence: f64,
    pub status: String,
    pub decision_reason: Option<String>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub source_quality: String,
    pub freshness: String,
    pub source_updated_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScholarEvidenceForAnswer {
    pub id: i64,
    pub source_url: String,
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub evidence_text: String,
    pub confidence: f64,
    pub gate_status: String,
    pub reason: String,
    pub research_relevance: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScholarSourceAssessment {
    pub finding_id: i64,
    pub topic_class: String,
    pub source_kind: String,
    pub topical_expertise: String,
    pub direct_support: Option<bool>,
    pub independence_key: Option<String>,
    pub independence_verified: bool,
    pub currency: String,
    pub published_at: Option<String>,
    pub gate_status: String,
    pub reason: String,
    pub research_relevance: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScholarSourceExpertiseReview {
    pub id: i64,
    pub finding_id: i64,
    pub source_kind: String,
    pub suitable_for_topic: bool,
    pub published_at: Option<String>,
    pub evidence_text: String,
    pub rationale: String,
    pub review_method: String,
    pub reviewed_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScholarSourceOriginReview {
    pub id: i64,
    pub finding_id: i64,
    pub underlying_source_id: String,
    pub evidence_text: String,
    pub rationale: String,
    pub review_method: String,
    pub reviewed_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssistantReplyJob {
    pub id: String,
    pub reply_text: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AssistantClaim {
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub evidence_text: String,
    pub extraction_confidence: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClaimJudgment {
    pub claim: AssistantClaim,
    pub verdict: String,
    pub challenge_type: Option<String>,
    pub reason: String,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredAssistantClaim {
    pub id: i64,
    pub reply_id: String,
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub evidence_text: String,
    pub verdict: String,
    pub reason: String,
    pub evidence_refs: Vec<String>,
    pub confidence_before: f64,
    pub confidence_after: f64,
}

mod assistant_audit;
mod candidate_review;
mod evidence_policy;
mod maintenance;
mod model_lineage;
mod observations;
mod operations;
mod queries;
mod research;
mod schema;

impl LearningStore {
    pub fn open(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }

        let connection = Connection::open(path).map_err(database_error)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(database_error)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(database_error)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(database_error)?;

        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }
}

fn link_claim_evidence(
    tx: &Transaction<'_>,
    claim_id: i64,
    source_kind: &str,
    source_uri: Option<&str>,
    observation_id: Option<&str>,
    excerpt: &str,
    stance: &str,
) -> io::Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO claim_evidence
            (source_kind, source_uri, observation_id, excerpt, retrieved_at, created_at)
         VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        params![source_kind, source_uri, observation_id, excerpt],
    )
    .map_err(database_error)?;
    let evidence_id: i64 = tx
        .query_row(
            "SELECT id FROM claim_evidence WHERE source_kind = ?1
             AND source_uri IS ?2 AND observation_id IS ?3 AND excerpt = ?4",
            params![source_kind, source_uri, observation_id, excerpt],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    tx.execute(
        "INSERT OR IGNORE INTO claim_evidence_links (claim_id, evidence_id, stance, linked_at)
         VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        params![claim_id, evidence_id, stance],
    )
    .map_err(database_error)?;
    Ok(())
}

struct RoleProposal<'a> {
    role: &'a str,
    source_type: &'a str,
    source_id: &'a str,
    suggested_action: &'a str,
    target_claim_id: Option<i64>,
    evidence_refs: &'a str,
    payload: &'a str,
}

fn record_role_proposal(tx: &Transaction<'_>, proposal: RoleProposal<'_>) -> io::Result<i64> {
    tx.execute(
        "INSERT INTO role_proposals
         (role, source_type, source_id, suggested_action, target_claim_id, evidence_refs, payload, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        params![
            proposal.role,
            proposal.source_type,
            proposal.source_id,
            proposal.suggested_action,
            proposal.target_claim_id,
            proposal.evidence_refs,
            proposal.payload,
        ],
    ).map_err(database_error)?;
    Ok(tx.last_insert_rowid())
}

fn record_policy_decision(
    tx: &Transaction<'_>,
    proposal_id: i64,
    outcome: &str,
    reason: &str,
    resulting_claim_id: Option<i64>,
    resulting_state: Option<&str>,
) -> io::Result<()> {
    tx.execute(
        "INSERT INTO policy_decisions
         (proposal_id, outcome, reason, resulting_claim_id, resulting_state, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        params![
            proposal_id,
            outcome,
            reason,
            resulting_claim_id,
            resulting_state
        ],
    )
    .map_err(database_error)?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub struct ImportSummary {
    pub inserted: usize,
    pub already_present: usize,
}

fn curate_candidate(
    tx: &Transaction<'_>,
    observation_id: &str,
    candidate: &mut CandidateClaim,
) -> io::Result<Result<i64, String>> {
    let mut statement = tx
        .prepare(
            "SELECT id, subject, relation, object, valid_from, valid_until FROM semantic_claims
         WHERE retracted_by IS NULL ORDER BY id",
        )
        .map_err(database_error)?;
    let existing = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(statement);
    let today: String = tx
        .query_row("SELECT date('now')", [], |row| row.get(0))
        .map_err(database_error)?;
    for (id, subject, relation, object, valid_from, valid_until) in existing {
        let overlaps = claim_periods_overlap(
            candidate.valid_from.as_deref(),
            candidate.valid_until.as_deref(),
            valid_from.as_deref(),
            valid_until.as_deref(),
            &today,
        );
        if !overlaps {
            continue;
        }
        if canonical(&subject) == canonical(&candidate.subject)
            && canonical(&relation) == canonical(&candidate.relation)
            && canonical(&object) == canonical(&candidate.object)
            && valid_from.as_deref() == candidate.valid_from.as_deref()
            && valid_until.as_deref() == candidate.valid_until.as_deref()
        {
            candidate.status = "promoted".into();
            candidate.decision_reason = Some("equivalent active claim; source attached".into());
            return Ok(Ok(id));
        }
        if canonical(&subject) == canonical(&candidate.subject)
            && canonical(&relation) == canonical(&candidate.relation)
            && canonical(&object) != canonical(&candidate.object)
        {
            return Ok(Err(format!(
                "conflicts with active semantic claim #{id}: {} {} {}",
                subject, relation, object
            )));
        }
    }
    tx.execute(
        "INSERT INTO semantic_claims
            (subject, relation, object, source, supersedes_id, retracted_by,
             source_observation_id, created_at, observed_at, valid_from, valid_until)
         VALUES (?1, ?2, ?3, ?4, NULL, NULL, ?5,
                 strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 (SELECT created_at FROM observations WHERE id = ?5), ?6, ?7)",
        params![
            candidate.subject,
            candidate.relation,
            candidate.object,
            format!("user observation {observation_id}"),
            observation_id,
            candidate.valid_from,
            candidate.valid_until,
        ],
    )
    .map_err(database_error)?;
    let id = tx
        .query_row("SELECT last_insert_rowid()", [], |row| row.get(0))
        .map_err(database_error)?;
    candidate.status = "promoted".into();
    candidate.decision_reason = None;
    Ok(Ok(id))
}

fn attach_equivalent_sources(
    tx: &Transaction<'_>,
    semantic_id: i64,
    current_candidate_id: &str,
    candidate: &CandidateClaim,
) -> io::Result<()> {
    let mut statement = tx
        .prepare(
            "SELECT id, observation_id FROM candidate_claims
             WHERE lower(trim(subject)) = lower(?1)
               AND lower(trim(relation)) = lower(?2)
               AND lower(trim(object)) = lower(?3)
               AND valid_from IS ?4 AND valid_until IS ?5
               AND status IN ('proposed', 'needs_review', 'promoted')",
        )
        .map_err(database_error)?;
    let candidates = statement
        .query_map(
            params![
                candidate.subject,
                candidate.relation,
                candidate.object,
                candidate.valid_from,
                candidate.valid_until,
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(statement);
    let includes_current = candidates.iter().any(|(id, _)| id == current_candidate_id);
    for (candidate_id, source_observation_id) in candidates {
        tx.execute(
            "INSERT OR IGNORE INTO semantic_claim_sources
                (semantic_claim_id, candidate_id, observation_id)
             VALUES (?1, ?2, ?3)",
            params![semantic_id, candidate_id, source_observation_id],
        )
        .map_err(database_error)?;
        let excerpt: String = tx
            .query_row(
                "SELECT raw_text FROM observations WHERE id = ?1",
                [&source_observation_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        link_claim_evidence(
            tx,
            semantic_id,
            "user_observation",
            None,
            Some(&source_observation_id),
            &excerpt,
            "supports",
        )?;
    }
    if !includes_current {
        return Err(invalid("promoted candidate is missing from its source set"));
    }
    Ok(())
}

fn normalize(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn canonical(value: &str) -> String {
    normalize(value).to_lowercase()
}

fn is_time_sensitive(question: &str, relation: &str, object: &str) -> bool {
    let text = format!("{question} {relation} {object}").to_lowercase();
    [
        "current",
        "currently",
        "latest",
        "today",
        "now",
        "price",
        "population",
        "president",
        "prime minister",
        "ceo",
        "version",
        "weather",
        "election",
        "stock market",
        "stock quote",
        "stock ticker",
        "stock symbol",
        "share price",
        "exchange rate",
        "market cap",
        "operating system",
        "gpu",
        "graphics card",
        "processor",
        "chipset",
        "specification",
    ]
    .iter()
    .any(|term| text.contains(term))
}

fn is_consequential(question: &str, subject: &str, relation: &str) -> bool {
    let text = format!("{question} {subject} {relation}").to_lowercase();
    [
        "medical",
        "medicine",
        "diagnos",
        "treatment",
        "symptom",
        "dose",
        "drug",
        "legal",
        "law",
        "rights",
        "court",
        "tax",
        "invest",
        "financial",
        "emergency",
        "safety",
        "weapon",
    ]
    .iter()
    .any(|term| text.contains(term))
}

pub(crate) fn valid_claim_period(valid_from: Option<&str>, valid_until: Option<&str>) -> bool {
    let valid_date = |value: &str| {
        let bytes = value.as_bytes();
        if bytes.len() != 10
            || bytes[4] != b'-'
            || bytes[7] != b'-'
            || !bytes[..4].iter().all(u8::is_ascii_digit)
            || !bytes[5..7].iter().all(u8::is_ascii_digit)
            || !bytes[8..].iter().all(u8::is_ascii_digit)
        {
            return false;
        }
        let Some(year) = value[..4].parse::<u32>().ok() else {
            return false;
        };
        let Some(month) = value[5..7].parse::<u32>().ok() else {
            return false;
        };
        let Some(day) = value[8..].parse::<u32>().ok() else {
            return false;
        };
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let days = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return false,
        };
        day > 0 && day <= days
    };
    if valid_from.is_some_and(|date| !valid_date(date))
        || valid_until.is_some_and(|date| !valid_date(date))
    {
        return false;
    }
    match (valid_from, valid_until) {
        (Some(from), Some(until)) => from <= until,
        _ => true,
    }
}

pub(crate) fn claim_periods_overlap(
    left_from: Option<&str>,
    left_until: Option<&str>,
    right_from: Option<&str>,
    right_until: Option<&str>,
    today: &str,
) -> bool {
    let (left_from, left_until) = if left_from.is_none() && left_until.is_none() {
        (Some(today), Some(today))
    } else {
        (left_from, left_until)
    };
    let (right_from, right_until) = if right_from.is_none() && right_until.is_none() {
        (Some(today), Some(today))
    } else {
        (right_from, right_until)
    };
    !matches!((left_until, right_from), (Some(until), Some(from)) if until < from)
        && !matches!((right_until, left_from), (Some(until), Some(from)) if until < from)
}

fn read_assistant_claim(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredAssistantClaim> {
    let refs: String = row.get(8)?;
    let evidence_refs = serde_json::from_str(&refs).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(StoredAssistantClaim {
        id: row.get(0)?,
        reply_id: row.get(1)?,
        subject: row.get(2)?,
        relation: row.get(3)?,
        object: row.get(4)?,
        evidence_text: row.get(5)?,
        verdict: row.get(6)?,
        reason: row.get(7)?,
        evidence_refs,
        confidence_before: row.get(9)?,
        confidence_after: row.get(10)?,
    })
}

pub(crate) fn evidence_supports_claim(finding: &ScholarFinding) -> bool {
    let evidence = canonical(&finding.evidence_text);
    if !evidence.contains(&canonical(&finding.subject))
        || !evidence_supports_object(&evidence, finding)
    {
        return false;
    }
    // Extractors sometimes emit schema-style names for ordinary copulas.
    let relation = canonical(&finding.relation).replace('_', " ");
    let relation_terms = relation
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| word.len() >= 4)
        .filter(|word| !matches!(*word, "from" | "into" | "that" | "with"));
    relation_terms
        .into_iter()
        .any(|word| evidence.contains(word))
        || relation.split_whitespace().all(|word| word.len() < 4)
}

fn evidence_supports_object(evidence: &str, finding: &ScholarFinding) -> bool {
    let object = canonical(&finding.object);
    if evidence.contains(&object) {
        return true;
    }
    finding.relation.to_ascii_lowercase().starts_with("lists ")
        && finding
            .object
            .split(';')
            .map(canonical)
            .filter(|item| !item.is_empty())
            .all(|item| evidence.contains(&item))
}

fn get_claim(tx: &Transaction<'_>, id: u64) -> io::Result<Option<Claim>> {
    let id = i64::try_from(id).map_err(|_| invalid("claim ID exceeds SQLite's integer range"))?;
    let row = tx
        .query_row(
            "SELECT id, subject, relation, object, source, supersedes_id, retracted_by
         FROM semantic_claims WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?;
    row.map(
        |(id, subject, relation, object, source, supersedes, retracted_by)| {
            Ok(Claim {
                id: u64::try_from(id).map_err(|_| invalid("SQLite claim ID is negative"))?,
                subject,
                relation,
                object,
                source,
                supersedes: supersedes
                    .map(u64::try_from)
                    .transpose()
                    .map_err(|_| invalid("SQLite superseded claim ID is negative"))?,
                retracted_by,
            })
        },
    )
    .transpose()
}

fn claims_equal(left: &Claim, right: &Claim) -> bool {
    left.id == right.id
        && left.subject == right.subject
        && left.relation == right.relation
        && left.object == right.object
        && left.source == right.source
        && left.supersedes == right.supersedes
        && left.retracted_by == right.retracted_by
}

fn database_error(error: rusqlite::Error) -> io::Error {
    io::Error::other(error)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn validate_review_reason(reason: &str) -> io::Result<()> {
    if reason.trim().is_empty()
        || reason.chars().count() > 500
        || reason.chars().any(char::is_control)
    {
        return Err(invalid(
            "review reason must contain 1 to 500 printable characters",
        ));
    }
    Ok(())
}

fn invalid_json(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

pub(crate) fn relevant_words(text: &str) -> std::collections::BTreeSet<String> {
    const STOP_WORDS: &[&str] = &[
        "a", "an", "and", "are", "did", "do", "does", "for", "from", "how", "i", "in", "is", "it",
        "me", "my", "of", "on", "or", "please", "tell", "that", "the", "to", "was", "what", "when",
        "where", "who", "why", "with", "you", "your", "than", "then", "be", "been", "being",
        "about", "this", "these", "those", "thanks", "thank",
    ];
    text.to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| word.chars().count() >= 2 && !STOP_WORDS.contains(word))
        .map(str::to_owned)
        .collect()
}

pub(crate) fn lexical_overlap(
    left: &std::collections::BTreeSet<String>,
    right: &std::collections::BTreeSet<String>,
) -> usize {
    left.iter()
        .filter(|word| {
            right.iter().any(|candidate| {
                word == &candidate
                    || (word.chars().count() >= 4
                        && candidate.chars().count() >= 4
                        && (word.starts_with(candidate.as_str())
                            || candidate.starts_with(word.as_str())))
            })
        })
        .count()
}

#[cfg(test)]
mod tests;
