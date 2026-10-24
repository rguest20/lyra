//! Shared storage test fixtures; behavioral regressions live in child modules.
use super::{
    AssistantClaim, CandidateClaim, ClaimJudgment, ClaimModality, ClaimPolarity,
    ConsolidatedClaimRevision, ImportSummary, LearningStore, ModelCheckpointManifest,
    PropositionMetadata, ScholarCandidateReview, ScholarEvidence, ScholarFinding,
    ScholarSearchCandidateInput, ScholarSourceProfile, direct_passage_supports_finding,
    evidence_supports_claim,
};
use crate::knowledge::KnowledgeMap;
use crate::test_support::{ScholarJobTestHelpers, scholar_finding};

fn scholar_search_candidate(
    result_type: &str,
    title: &str,
    url: &str,
    snippet: &str,
    structured_data_json: &str,
) -> ScholarSearchCandidateInput {
    ScholarSearchCandidateInput {
        result_type: result_type.into(),
        title: title.into(),
        url: url.into(),
        snippet: snippet.into(),
        structured_data_json: structured_data_json.into(),
    }
}

fn scholar_candidate_review(
    status: &'static str,
    policy_status: &'static str,
    policy_reason: Option<&str>,
    fetch_status: &'static str,
    fetched_url: Option<&str>,
    excerpt: Option<&str>,
) -> ScholarCandidateReview {
    ScholarCandidateReview {
        status,
        policy_status,
        policy_reason: policy_reason.map(str::to_owned),
        fetch_status,
        fetched_url: fetched_url.map(str::to_owned),
        excerpt: excerpt.map(str::to_owned),
        passages: Vec::new(),
        provenance_json: None,
    }
}

fn candidate_claim(
    subject: &str,
    relation: &str,
    object: &str,
    evidence: &str,
    confidence: f64,
) -> CandidateClaim {
    CandidateClaim {
        subject: subject.into(),
        relation: relation.into(),
        object: object.into(),
        evidence_text: evidence.into(),
        context_message_ids: Vec::new(),
        resolution: None,
        confidence,
        status: "proposed".into(),
        decision_reason: None,
        valid_from: None,
        valid_until: None,
    }
}

fn review_candidate_claim(
    subject: &str,
    relation: &str,
    object: &str,
    evidence: &str,
    reason: &str,
) -> CandidateClaim {
    let mut claim = candidate_claim(subject, relation, object, evidence, 0.0);
    claim.status = "needs_review".into();
    claim.decision_reason = Some(reason.into());
    claim
}

#[test]
fn printable_and_bounded_fields_keep_their_distinct_token_marker_rules() {
    assert!(super::valid_printable_field("review text <|marker", 30));
    assert!(!super::valid_bounded_field("review text <|marker", 30));
    assert!(!super::valid_printable_field("  ", 30));
    assert!(!super::valid_printable_field("line\nbreak", 30));
    assert!(!super::valid_printable_field("too long", 3));
}

#[test]
fn publication_recency_uses_one_year_window_without_future_or_invalid_dates() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    let today: String = connection
        .query_row("SELECT date('now')", [], |row| row.get(0))
        .unwrap();
    let cutoff: String = connection
        .query_row("SELECT date('now', '-12 months')", [], |row| row.get(0))
        .unwrap();
    let future: String = connection
        .query_row("SELECT date('now', '+1 day')", [], |row| row.get(0))
        .unwrap();

    assert!(super::publication_date_is_recent(&connection, Some(&today)).unwrap());
    assert!(!super::publication_date_is_recent(&connection, Some(&cutoff)).unwrap());
    assert!(!super::publication_date_is_recent(&connection, Some(&future)).unwrap());
    assert!(!super::publication_date_is_recent(&connection, Some("not a date")).unwrap());
    assert!(!super::publication_date_is_recent(&connection, None).unwrap());
}

#[test]
fn claim_period_requires_each_valid_bound_to_appear_in_its_evidence() {
    let evidence = "From 2010-01-01 through 2015-12-31.";
    assert!(super::claim_period_is_stated(
        Some("2010-01-01"),
        Some("2015-12-31"),
        evidence
    ));
    assert!(!super::claim_period_is_stated(
        Some("2010-01-01"),
        Some("2016-12-31"),
        evidence
    ));
    assert!(!super::claim_period_is_stated(
        Some("not a date"),
        None,
        evidence
    ));
    assert!(super::claim_period_is_stated(None, None, evidence));
}

fn temp_path() -> std::path::PathBuf {
    crate::test_support::unique_temp_path("community-learning", "db")
}

fn feedback_fixture(store: &mut LearningStore) -> (String, i64) {
    store
        .enqueue_research("feedback-test", 1, "What kind of body is Mars?")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let id = feedback_finding(store, &job.id);
    (job.id, id)
}

fn feedback_finding(store: &mut LearningStore, job: &str) -> i64 {
    store
        .finish_scholar_job(
            job,
            "Mars",
            "https://evidence.example/mars",
            "Mars is a planet.",
            Some(scholar_finding(
                "Mars",
                "is a",
                "planet",
                "Mars is a planet.",
                0.99,
                "unknown",
                None,
            )),
            None,
        )
        .unwrap();
    store.scholar_findings(job).unwrap()[0].id
}

fn semantic_round_finding() -> ScholarFinding {
    let mut finding = crate::semantic::candidate_from_context(&serde_json::json!({"subject":"The world","relation":"is","object":"round","evidence_text":"The world is round."})).unwrap();
    let passage = "The world is spherical.";
    let support = crate::semantic::assess_with("What shape is the world?","World",passage,&finding, |_|Ok(serde_json::json!({"verdict":"entailed","reason":"Spherical entails round.","evidence_text":passage}).to_string())).unwrap();
    finding.evidence_text = passage.into();
    finding.semantic_support = Some(support);
    finding
}

mod claims_observations;
mod feedback;
mod identity_scope;
mod maintenance;
mod migrations;
mod network;
mod provenance;
mod scheduling;
mod semantic_support;
mod source_grounding;
mod source_review;
mod training;
