//! Local SQLite storage for staged learning and semantic-map claims.

use std::fs;
use std::io;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

pub(crate) use crate::errors::invalid_data as invalid;
use crate::knowledge::{Claim, KnowledgeMap};

const SCHEMA_VERSION: i64 = 63;
static SCHEMA_MIGRATION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) struct ResearchSourceInput<'a> {
    pub job_id: &'a str,
    pub title: &'a str,
    pub url: &'a str,
    pub body: &'a str,
    pub provider: &'a str,
    pub source_scope: &'a str,
    pub query: Option<&'a str>,
    pub published_at: Option<&'a str>,
}

pub(crate) fn unix_time_millis() -> io::Result<u128> {
    research::unix_time_millis().map_err(|error| io::Error::other(error.to_string()))
}

pub(crate) fn unix_time_ms_i64_with_overflow(
    overflow: impl FnOnce(std::num::TryFromIntError) -> io::Error,
) -> io::Result<i64> {
    let timestamp = unix_time_millis()?;
    i64::try_from(timestamp).map_err(overflow)
}

pub(crate) fn normalize_domain(value: &str) -> String {
    let domain = value.trim().trim_end_matches('.').to_ascii_lowercase();
    domain.strip_prefix("www.").unwrap_or(&domain).to_owned()
}

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

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ScholarAiReviewTask {
    pub finding_id: i64,
    pub job_id: String,
}

#[cfg(test)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScholarTaskTarget {
    pub job_id: String,
    pub original_question: String,
    pub entity_constraints: Vec<String>,
    pub scope_constraints: Vec<String>,
    pub evidence_gap: String,
    pub acceptable_evidence: Vec<String>,
    pub sources_tried: Vec<ScholarSearchAttempt>,
    pub stop_reason: Option<String>,
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

impl Observation {
    pub(crate) fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let turn_number: i64 = row.get(2)?;
        Ok(Self {
            id: row.get(0)?,
            conversation_id: row.get(1)?,
            turn_number: usize::try_from(turn_number)
                .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(2, turn_number))?,
            raw_text: row.get(3)?,
            status: row.get(4)?,
            created_at: row.get(5)?,
        })
    }
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
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
pub struct DeferredModelProbe {
    pub id: i64,
    pub seed: String,
    pub topic: String,
    pub question: String,
    pub raw_output: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaimTopic {
    pub id: i64,
    pub subject: String,
    pub relation: String,
    pub object: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScholarSearchAttempt {
    pub provider: String,
    pub trigger_reason: String,
    pub query: String,
    pub status: String,
    pub result_count: Option<usize>,
    pub error: Option<String>,
}

pub(crate) struct ScholarSearchAttemptInput<'a> {
    pub job_id: &'a str,
    pub provider: &'a str,
    pub trigger_reason: &'a str,
    pub query: &'a str,
    pub status: &'a str,
    pub result_count: Option<usize>,
    pub error: Option<&'a str>,
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
pub(crate) struct CachedSitePolicy {
    pub status: u16,
    pub content_type: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CachedSourcePage {
    pub content_type: String,
    pub body: String,
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
pub(crate) struct ScholarSearchCandidateInput {
    pub result_type: String,
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub structured_data_json: String,
}

pub(crate) struct ScholarCandidateReview {
    pub status: &'static str,
    pub policy_status: &'static str,
    pub policy_reason: Option<String>,
    pub fetch_status: &'static str,
    pub fetched_url: Option<String>,
    pub excerpt: Option<String>,
    pub passages: Vec<String>,
    pub provenance_json: Option<String>,
}

impl ScholarCandidateReview {
    pub(crate) fn skipped(policy_status: &'static str, reason: impl Into<String>) -> Self {
        Self::skipped_with_url(policy_status, reason, None)
    }

    pub(crate) fn skipped_with_url(
        policy_status: &'static str,
        reason: impl Into<String>,
        fetched_url: Option<String>,
    ) -> Self {
        Self {
            status: "skipped",
            policy_status,
            policy_reason: Some(reason.into()),
            fetch_status: "skipped",
            fetched_url,
            excerpt: None,
            passages: Vec::new(),
            provenance_json: None,
        }
    }

    pub(crate) fn failed(reason: impl Into<String>) -> Self {
        Self {
            status: "failed",
            policy_status: "unavailable",
            policy_reason: Some(reason.into()),
            fetch_status: "failed",
            fetched_url: None,
            excerpt: None,
            passages: Vec::new(),
            provenance_json: None,
        }
    }

    pub(crate) fn fetch_failed(reason: impl Into<String>) -> Self {
        let mut review = Self::failed(format!(
            "site policy checks allowed the candidate fetch; {}",
            reason.into()
        ));
        review.policy_status = "allowed";
        review
    }
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
    pub scout_bucket_questions_pending: usize,
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
    #[serde(default)]
    pub approval_reason: String,
    #[serde(default)]
    pub approved_at: String,
    #[serde(default)]
    pub evidence: Vec<ClaimEvidence>,
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
    pub proposition: PropositionMetadata,
}

/// Explicit linguistic scope extracted from the passage. Unknown values stay
/// unknown; these fields do not assert that the proposition is true.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PropositionMetadata {
    pub attribution: Option<String>,
    pub geographic_scope: Option<String>,
    pub context_scope: Option<String>,
    pub polarity: ClaimPolarity,
    pub modality: ClaimModality,
}

impl PropositionMetadata {
    pub(crate) fn from_storage_row(
        row: &rusqlite::Row<'_>,
        first_column: usize,
    ) -> rusqlite::Result<Self> {
        Ok(Self {
            attribution: row.get(first_column)?,
            polarity: ClaimPolarity::from_storage_value(&row.get::<_, String>(first_column + 1)?),
            modality: ClaimModality::from_storage_value(&row.get::<_, String>(first_column + 2)?),
            geographic_scope: row.get(first_column + 3)?,
            context_scope: row.get(first_column + 4)?,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClaimPolarity {
    Asserted,
    Negated,
    #[default]
    Unknown,
}

impl ClaimPolarity {
    pub(crate) fn from_storage_value(value: &str) -> Self {
        match value {
            "asserted" => Self::Asserted,
            "negated" => Self::Negated,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Asserted => "asserted",
            Self::Negated => "negated",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClaimModality {
    Factual,
    Possible,
    Necessary,
    Conditional,
    #[default]
    Unknown,
}

impl ClaimModality {
    pub(crate) fn from_storage_value(value: &str) -> Self {
        match value {
            "factual" => Self::Factual,
            "possible" => Self::Possible,
            "necessary" => Self::Necessary,
            "conditional" => Self::Conditional,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Factual => "factual",
            Self::Possible => "possible",
            Self::Necessary => "necessary",
            Self::Conditional => "conditional",
            Self::Unknown => "unknown",
        }
    }
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

impl ScholarEvidence {
    pub(crate) fn new(
        source_title: impl Into<String>,
        source_url: impl Into<String>,
        source_excerpt: impl Into<String>,
        finding: Option<ScholarFinding>,
        research_relevance: impl Into<String>,
    ) -> Self {
        Self {
            source_title: source_title.into(),
            source_url: source_url.into(),
            source_excerpt: source_excerpt.into(),
            finding,
            profile: ScholarSourceProfile::default(),
            research_relevance: research_relevance.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScholarEvidenceSpan {
    pub source_archive_id: Option<i64>,
    pub evidence_byte_start: Option<i64>,
    pub evidence_byte_end: Option<i64>,
    pub source_url: String,
    pub evidence_text: String,
}

impl ScholarEvidenceSpan {
    pub(crate) fn from_row(
        row: &rusqlite::Row<'_>,
        first_column: usize,
        evidence_text_column: usize,
    ) -> rusqlite::Result<Self> {
        Ok(Self {
            source_archive_id: row.get(first_column)?,
            evidence_byte_start: row.get(first_column + 1)?,
            evidence_byte_end: row.get(first_column + 2)?,
            source_url: row.get(first_column + 3)?,
            evidence_text: row.get(evidence_text_column)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredScholarFinding {
    pub id: i64,
    pub source_span: ScholarEvidenceSpan,
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub confidence: f64,
    pub status: String,
    pub decision_reason: Option<String>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub source_quality: String,
    pub freshness: String,
    pub source_updated_at: Option<String>,
    pub proposition: PropositionMetadata,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScholarEvidenceForAnswer {
    pub id: i64,
    pub source_span: ScholarEvidenceSpan,
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub proposition: PropositionMetadata,
    pub confidence: f64,
    pub finding_status: String,
    pub gate_status: String,
    pub reason: String,
    pub research_relevance: String,
    pub topic_class: String,
    pub source_kind: String,
    pub freshness: String,
    pub currency: String,
    pub direct_support: bool,
    pub independence_key: Option<String>,
    pub independence_verified: bool,
    pub published_at: Option<String>,
    pub conflicting_claims: Vec<String>,
}

impl std::ops::Deref for StoredScholarFinding {
    type Target = ScholarEvidenceSpan;

    fn deref(&self) -> &Self::Target {
        &self.source_span
    }
}

impl std::ops::Deref for ScholarEvidenceForAnswer {
    type Target = ScholarEvidenceSpan;

    fn deref(&self) -> &Self::Target {
        &self.source_span
    }
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AssistantReplyAttention {
    pub reply_id: String,
    pub status: String,
    pub error: String,
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
mod entities;
pub(crate) use entities::EntityAliases;
mod evidence_policy;
mod maintenance;
mod model_lineage;
mod observations;
mod operations;
mod queries;
mod research;
pub(crate) use research::ScoutAdmission;
pub(crate) use research::baskets::{ClaimPair, ResearchBaskets, ResearchSource};
pub(crate) use research::network::{MEDIAWIKI_TRANSIENT_ERROR, wikipedia_interval_ms};
mod schema;

impl LearningStore {
    pub fn open(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }

        let _migration_guard = SCHEMA_MIGRATION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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

fn review_decision_outcome(method: &str, accepted: bool) -> &'static str {
    match (method == "ai", accepted) {
        (true, true) => "ai_accepted",
        (true, false) => "ai_rejected",
        (false, true) => "human_accepted",
        (false, false) => "human_rejected",
    }
}

fn record_policy_decision(
    tx: &Transaction<'_>,
    proposal_id: i64,
    outcome: &str,
    reason: &str,
    resulting_claim_id: Option<i64>,
    resulting_state: Option<&str>,
) -> io::Result<()> {
    let decision_method = if outcome.starts_with("ai_") {
        "ai"
    } else if outcome.starts_with("human_") {
        "human"
    } else {
        "deterministic_policy"
    };
    let model_id = (decision_method == "ai")
        .then(|| std::env::var("COMMUNITY_MODEL_ID").ok())
        .flatten();
    let model_version = (decision_method == "ai")
        .then(|| std::env::var("COMMUNITY_MODEL_VERSION").ok())
        .flatten();
    tx.execute(
        "INSERT INTO policy_decisions
         (proposal_id, outcome, reason, resulting_claim_id, resulting_state, created_at,
          decision_method, model_id, model_version)
         VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?6, ?7, ?8)",
        params![
            proposal_id,
            outcome,
            reason,
            resulting_claim_id,
            resulting_state,
            decision_method,
            model_id,
            model_version
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

pub(super) struct ActiveSemanticClaim {
    id: i64,
    subject: String,
    relation: String,
    object: String,
    valid_from: Option<String>,
    valid_until: Option<String>,
}

pub(super) fn link_candidate_to_semantic_claim(
    tx: &Transaction<'_>,
    semantic_claim_id: i64,
    candidate_id: &str,
    observation_id: &str,
) -> io::Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO semantic_claim_sources
            (semantic_claim_id, candidate_id, observation_id) VALUES (?1, ?2, ?3)",
        params![semantic_claim_id, candidate_id, observation_id],
    )
    .map_err(database_error)?;
    Ok(())
}

impl ActiveSemanticClaim {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            subject: row.get(1)?,
            relation: row.get(2)?,
            object: row.get(3)?,
            valid_from: row.get(4)?,
            valid_until: row.get(5)?,
        })
    }
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
        .query_map([], ActiveSemanticClaim::from_row)
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(statement);
    let today = current_sql_date(tx)?;
    let aliases = entities::EntityAliases::load(tx)?;
    for claim in existing {
        let overlaps = claim_periods_overlap(
            candidate.valid_from.as_deref(),
            candidate.valid_until.as_deref(),
            claim.valid_from.as_deref(),
            claim.valid_until.as_deref(),
            &today,
        );
        if !overlaps {
            continue;
        }
        if aliases.same(&claim.subject, &candidate.subject)
            && canonical(&claim.relation) == canonical(&candidate.relation)
            && canonical(&claim.object) == canonical(&candidate.object)
            && claim.valid_from.as_deref() == candidate.valid_from.as_deref()
            && claim.valid_until.as_deref() == candidate.valid_until.as_deref()
        {
            candidate.status = "promoted".into();
            candidate.decision_reason = Some("equivalent active claim; source attached".into());
            return Ok(Ok(claim.id));
        }
        if aliases.same(&claim.subject, &candidate.subject)
            && canonical(&claim.relation) == canonical(&candidate.relation)
            && canonical(&claim.object) != canonical(&candidate.object)
        {
            return Ok(Err(format!(
                "conflicts with active semantic claim #{}: {} {} {}",
                claim.id, claim.subject, claim.relation, claim.object
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
    let id = tx.last_insert_rowid();
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
             WHERE (lower(trim(subject)) = lower(?1) OR EXISTS (SELECT 1 FROM entity_equivalences e WHERE e.left_name = lower(trim(subject)) AND e.right_name = lower(?1)))
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
        link_candidate_to_semantic_claim(tx, semantic_id, &candidate_id, &source_observation_id)?;
        let excerpt = observations::read_observation_text(tx, &source_observation_id)?;
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
        "recent",
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
    .any(|term| {
        if matches!(*term, "now" | "current" | "today" | "latest") {
            text.split(|ch: char| !ch.is_alphanumeric())
                .any(|word| word == *term)
        } else {
            contains_policy_term(&text, term)
        }
    })
}

fn contains_policy_term(text: &str, term: &str) -> bool {
    let words = text
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let terms = term.split_whitespace().collect::<Vec<_>>();
    words.windows(terms.len()).any(|window| {
        window
            .iter()
            .zip(&terms)
            .all(|(word, term)| policy_word_match(word, term))
    })
}

fn policy_word_match(word: &str, term: &str) -> bool {
    word == term
        || word.strip_suffix('s') == Some(term)
        || word.strip_suffix("es") == Some(term)
        || term
            .strip_suffix('y')
            .is_some_and(|root| word == format!("{root}ies"))
        || (term.chars().count() >= 5
            && ["ed", "er", "ing", "is", "ly", "ment", "or"]
                .iter()
                .any(|suffix| {
                    let inflected = format!("{term}{suffix}");
                    word == inflected
                        || word.strip_suffix('s') == Some(inflected.as_str())
                        || word.strip_suffix("es") == Some(inflected.as_str())
                }))
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
    .any(|term| contains_policy_term(&text, term))
}

pub(crate) fn valid_printable_field(value: &str, maximum_chars: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= maximum_chars
        && !value.chars().any(char::is_control)
}

pub(crate) fn valid_bounded_field(value: &str, maximum_chars: usize) -> bool {
    valid_printable_field(value, maximum_chars) && !value.contains("<|")
}

pub(crate) fn valid_quoted_scope(scope: &str, source_text: &str, evidence_text: &str) -> bool {
    valid_quoted_field(scope, evidence_text, 240) && source_text.contains(scope)
}

pub(crate) fn valid_quoted_field(value: &str, evidence_text: &str, maximum_chars: usize) -> bool {
    valid_printable_field(value, maximum_chars) && evidence_text.contains(value)
}

pub(crate) fn valid_exact_quote(quote: &str, source_text: &str, maximum_chars: usize) -> bool {
    valid_source_quote(quote, source_text) && quote.chars().count() <= maximum_chars
}

pub(crate) fn valid_source_quote(quote: &str, source_text: &str) -> bool {
    !quote.trim().is_empty() && source_text.contains(quote)
}

pub(crate) fn valid_proposition_fields(subject: &str, relation: &str, object: &str) -> bool {
    [subject, relation, object]
        .into_iter()
        .all(|field| valid_bounded_field(field, 500))
}

pub(crate) fn valid_proposition_evidence(
    subject: &str,
    relation: &str,
    object: &str,
    quote: &str,
    source_text: &str,
) -> bool {
    valid_proposition_fields(subject, relation, object) && valid_source_quote(quote, source_text)
}

pub(crate) fn valid_confidence(confidence: f64) -> bool {
    confidence.is_finite() && (0.0..=1.0).contains(&confidence)
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

pub(crate) fn claim_period_is_stated(
    valid_from: Option<&str>,
    valid_until: Option<&str>,
    text: &str,
) -> bool {
    valid_claim_period(valid_from, valid_until)
        && [valid_from, valid_until]
            .into_iter()
            .flatten()
            .all(|date| text.contains(date))
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

pub(crate) fn direct_passage_supports_finding(
    source_title: &str,
    finding: &ScholarFinding,
) -> bool {
    evidence_supports_claim(finding)
        || (matches!(
            finding.relation.as_str(),
            "has encyclopedia summary" | "has encyclopedia detail"
        ) && canonical(&finding.subject) == canonical(source_title)
            && finding.object == finding.evidence_text)
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

pub(crate) fn current_sql_date(connection: &Connection) -> io::Result<String> {
    connection
        .query_row("SELECT date('now')", [], |row| row.get(0))
        .map_err(database_error)
}

pub(crate) fn publication_date_is_recent(
    connection: &Connection,
    published_at: Option<&str>,
) -> io::Result<bool> {
    let Some(published_at) = published_at else {
        return Ok(false);
    };
    connection
        .query_row(
            "SELECT COALESCE(date(?1) > date('now', '-12 months')
                        AND date(?1) <= date('now'), 0)",
            [published_at],
            |row| row.get(0),
        )
        .map_err(database_error)
}

fn database_error(error: rusqlite::Error) -> io::Error {
    io::Error::other(error)
}

fn sqlite_limit(limit: usize) -> io::Result<i64> {
    i64::try_from(limit).map_err(|_| invalid("limit too large"))
}

fn sqlite_count(count: i64) -> usize {
    usize::try_from(count).unwrap_or(usize::MAX)
}

fn validate_review_reason(reason: &str) -> io::Result<()> {
    require_printable_field(
        reason,
        500,
        "review reason must contain 1 to 500 printable characters",
    )
}

fn require_printable_field(
    value: &str,
    maximum_chars: usize,
    error_message: &'static str,
) -> io::Result<()> {
    if valid_printable_field(value, maximum_chars) {
        Ok(())
    } else {
        Err(invalid(error_message))
    }
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
mod grounding_tests;
#[cfg(test)]
mod policy_tests;
#[cfg(test)]
mod tests;
