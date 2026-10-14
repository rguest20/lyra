//! Shared storage test fixtures; behavioral regressions live in child modules.
use super::{
    AssistantClaim, CandidateClaim, ClaimJudgment, ClaimModality, ClaimPolarity,
    ConsolidatedClaimRevision, ImportSummary, LearningStore, ModelCheckpointManifest,
    PropositionMetadata, ScholarEvidence, ScholarFinding, ScholarSourceProfile,
    evidence_supports_claim,
};
use crate::knowledge::KnowledgeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_DB: AtomicUsize = AtomicUsize::new(0);

fn temp_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "community-learning-{}-{}.db",
        std::process::id(),
        NEXT_DB.fetch_add(1, Ordering::Relaxed)
    ))
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
            Some(ScholarFinding {
                semantic_support: None,
                subject: "Mars".into(),
                relation: "is a".into(),
                object: "planet".into(),
                evidence_text: "Mars is a planet.".into(),
                confidence: 0.99,
                valid_from: None,
                valid_until: None,
                source_quality: "unknown".into(),
                source_updated_at: None,
                proposition: Default::default(),
            }),
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
