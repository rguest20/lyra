use crate::storage::{
    LearningStore, PropositionMetadata, ScholarEvidence, ScholarEvidenceForAnswer,
    ScholarEvidenceSpan, ScholarFinding, ScholarSourceProfile,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) trait ScholarJobTestHelpers {
    fn finish_scholar_job_with_profile_and_relevance(
        &mut self,
        job_id: &str,
        source_title: &str,
        source_url: &str,
        source_excerpt: &str,
        finding: Option<ScholarFinding>,
        note: Option<&str>,
        profile: &ScholarSourceProfile,
        research_relevance: &str,
    ) -> std::io::Result<()>;

    fn finish_scholar_job(
        &mut self,
        job_id: &str,
        source_title: &str,
        source_url: &str,
        source_excerpt: &str,
        finding: Option<ScholarFinding>,
        note: Option<&str>,
    ) -> std::io::Result<()> {
        self.finish_scholar_job_with_profile(
            job_id,
            source_title,
            source_url,
            source_excerpt,
            finding,
            note,
            &ScholarSourceProfile::default(),
        )
    }

    fn finish_scholar_job_with_profile(
        &mut self,
        job_id: &str,
        source_title: &str,
        source_url: &str,
        source_excerpt: &str,
        finding: Option<ScholarFinding>,
        note: Option<&str>,
        profile: &ScholarSourceProfile,
    ) -> std::io::Result<()> {
        self.finish_scholar_job_with_profile_and_relevance(
            job_id,
            source_title,
            source_url,
            source_excerpt,
            finding,
            note,
            profile,
            "unknown",
        )
    }
}

impl ScholarJobTestHelpers for LearningStore {
    fn finish_scholar_job_with_profile_and_relevance(
        &mut self,
        job_id: &str,
        source_title: &str,
        source_url: &str,
        source_excerpt: &str,
        finding: Option<ScholarFinding>,
        note: Option<&str>,
        profile: &ScholarSourceProfile,
        research_relevance: &str,
    ) -> std::io::Result<()> {
        let mut evidence = ScholarEvidence::new(
            source_title,
            source_url,
            source_excerpt,
            finding,
            research_relevance,
        );
        evidence.profile = profile.clone();
        self.finish_scholar_job_with_evidence(job_id, &[evidence], note)
    }
}

static NEXT_TEMP_PATH: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn in_memory_learning_store() -> crate::storage::LearningStore {
    crate::storage::LearningStore::open(std::path::Path::new(":memory:")).unwrap()
}

pub(crate) fn scholar_finding(
    subject: &str,
    relation: &str,
    object: &str,
    evidence: &str,
    confidence: f64,
    source_quality: &str,
    source_updated_at: Option<&str>,
) -> ScholarFinding {
    ScholarFinding {
        semantic_support: None,
        subject: subject.into(),
        relation: relation.into(),
        object: object.into(),
        evidence_text: evidence.into(),
        confidence,
        valid_from: None,
        valid_until: None,
        source_quality: source_quality.into(),
        source_updated_at: source_updated_at.map(str::to_owned),
        proposition: PropositionMetadata::default(),
    }
}

pub(crate) fn unique_temp_path(prefix: &str, extension: &str) -> PathBuf {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = NEXT_TEMP_PATH.fetch_add(1, Ordering::Relaxed);
    let suffix = if extension.is_empty() {
        String::new()
    } else {
        format!(".{extension}")
    };
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{timestamp}-{sequence}{suffix}",
        std::process::id()
    ))
}

pub(crate) fn answer_evidence_fixture(
    id: i64,
    subject: &str,
    relation: &str,
    object: &str,
    source_url: &str,
    evidence_text: &str,
) -> ScholarEvidenceForAnswer {
    ScholarEvidenceForAnswer {
        id,
        source_span: ScholarEvidenceSpan {
            source_archive_id: None,
            evidence_byte_start: None,
            evidence_byte_end: None,
            source_url: source_url.into(),
            evidence_text: evidence_text.into(),
        },
        subject: subject.into(),
        relation: relation.into(),
        object: object.into(),
        proposition: PropositionMetadata::default(),
        confidence: 0.99,
        finding_status: "needs_review".into(),
        gate_status: "pending_review".into(),
        reason: "test".into(),
        research_relevance: "direct".into(),
        topic_class: "stable".into(),
        source_kind: "reference".into(),
        freshness: "fresh".into(),
        currency: "current".into(),
        direct_support: true,
        independence_key: Some("source-a".into()),
        independence_verified: true,
        published_at: None,
        conflicting_claims: Vec::new(),
        review_caveats: Vec::new(),
    }
}
