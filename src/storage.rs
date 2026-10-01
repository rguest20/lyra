//! Local SQLite storage for staged learning and semantic-map claims.

use std::fs;
use std::io;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::knowledge::{Claim, KnowledgeMap};

const SCHEMA_VERSION: i64 = 8;
const PROMOTION_CONFIDENCE: f64 = 0.95;
const ASSISTANT_BASE_CONFIDENCE: f64 = 0.5;
const SEVERE_CHALLENGE_MULTIPLIER: f64 = 0.2;

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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticClaim {
    pub id: i64,
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub source: String,
    pub source_observation_id: Option<String>,
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObservationDetail {
    pub observation: Observation,
    pub last_error: Option<String>,
    pub candidates: Vec<StoredCandidate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticClaimDetail {
    pub claim: SemanticClaim,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScholarJob {
    pub id: String,
    pub question: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoutInvestigation {
    pub trigger_kind: String,
    pub topic_key: String,
    pub trigger_ref: String,
    pub question: String,
    pub scholar_job_id: String,
}

pub struct ModelConsolidation {
    pub id: i64,
    pub claim_id: i64,
    pub source_checkpoint: String,
    pub output_checkpoint: String,
    pub training_loss: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScholarFinding {
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub evidence_text: String,
    pub confidence: f64,
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

impl LearningStore {
    pub fn stage_assistant_reply(
        &self,
        conversation_id: &str,
        turn_number: usize,
        reply_text: &str,
    ) -> io::Result<()> {
        let turn_number =
            i64::try_from(turn_number).map_err(|_| invalid("turn number too large"))?;
        self.connection
            .execute(
                "INSERT INTO assistant_replies
             (id, conversation_id, turn_number, reply_text, status, created_at)
             VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, 'pending',
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(conversation_id, turn_number) DO NOTHING",
                params![conversation_id, turn_number, reply_text],
            )
            .map_err(database_error)?;
        Ok(())
    }

    pub fn claim_assistant_reply(&mut self) -> io::Result<Option<AssistantReplyJob>> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let job = tx.query_row(
            "SELECT id, reply_text, status FROM assistant_replies
             WHERE status = 'pending' OR
               (status = 'processing' AND claimed_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-600 seconds'))
             ORDER BY created_at, rowid LIMIT 1",
            [],
            |row| Ok(AssistantReplyJob { id: row.get(0)?, reply_text: row.get(1)?, status: row.get(2)? }),
        ).optional().map_err(database_error)?;
        if let Some(job) = &job {
            tx.execute(
                "UPDATE assistant_replies SET status = 'processing',
                 claimed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
                [&job.id],
            )
            .map_err(database_error)?;
        }
        tx.commit().map_err(database_error)?;
        Ok(job)
    }

    pub fn finish_assistant_reply(
        &mut self,
        job_id: &str,
        judgments: &[ClaimJudgment],
    ) -> io::Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        for judgment in judgments {
            let confidence_before = ASSISTANT_BASE_CONFIDENCE;
            let confidence_after = if judgment.verdict == "challenged" {
                confidence_before * SEVERE_CHALLENGE_MULTIPLIER
            } else {
                confidence_before
            };
            tx.execute(
                "INSERT INTO assistant_claims
                 (reply_id, subject, relation, object, evidence_text, extraction_confidence,
                  confidence_before, confidence_after, verdict, challenge_type, reason, evidence_refs, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                         strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![job_id, judgment.claim.subject, judgment.claim.relation,
                    judgment.claim.object, judgment.claim.evidence_text,
                    judgment.claim.extraction_confidence, confidence_before,
                    confidence_after, judgment.verdict, judgment.challenge_type,
                    judgment.reason, serde_json::to_string(&judgment.evidence_refs).map_err(invalid_json)?],
            ).map_err(database_error)?;
        }
        let status = if judgments.iter().any(|item| item.verdict == "challenged") {
            "challenged"
        } else if judgments.is_empty() {
            "no_claims"
        } else {
            "inconclusive"
        };
        let changed = tx
            .execute(
                "UPDATE assistant_replies SET status = ?2, claimed_at = NULL, last_error = NULL
             WHERE id = ?1 AND status = 'processing'",
                params![job_id, status],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("assistant reply is not processing"));
        }
        tx.commit().map_err(database_error)
    }

    pub fn hold_assistant_reply(&self, job_id: &str, reason: &str) -> io::Result<()> {
        self.connection.execute(
            "UPDATE assistant_replies SET status = 'needs_review', claimed_at = NULL, last_error = ?2
             WHERE id = ?1 AND status = 'processing'",
            params![job_id, reason],
        ).map_err(database_error)?;
        Ok(())
    }

    pub fn assistant_claims(&self, limit: usize) -> io::Result<Vec<StoredAssistantClaim>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, reply_id, subject, relation, object, evidence_text,
                    verdict, reason, evidence_refs, confidence_before, confidence_after
             FROM assistant_claims ORDER BY id DESC LIMIT ?1",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map(
                [i64::try_from(limit).map_err(|_| invalid("limit too large"))?],
                read_assistant_claim,
            )
            .map_err(database_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(database_error)
    }

    pub fn assistant_reply_attention(&self) -> io::Result<Vec<(String, String, String)>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, status, COALESCE(last_error, '') FROM assistant_replies
             WHERE status IN ('pending', 'processing', 'needs_review')
             ORDER BY created_at DESC LIMIT 20",
            )
            .map_err(database_error)?;
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn relevant_assistant_challenges(
        &self,
        query: &str,
        limit: usize,
    ) -> io::Result<Vec<StoredAssistantClaim>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let terms = relevant_words(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, reply_id, subject, relation, object, evidence_text,
                    verdict, reason, evidence_refs, confidence_before, confidence_after
             FROM assistant_claims WHERE verdict = 'challenged'",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([], read_assistant_claim)
            .map_err(database_error)?;
        let mut claims = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
            .into_iter()
            .filter_map(|claim| {
                let content = format!("{} {} {}", claim.subject, claim.relation, claim.object);
                let score = lexical_overlap(&terms, &relevant_words(&content));
                (score > 0).then_some((score, claim))
            })
            .collect::<Vec<_>>();
        claims.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(claims
            .into_iter()
            .take(limit)
            .map(|(_, claim)| claim)
            .collect())
    }

    pub fn active_semantic_claim_ids(&self) -> io::Result<std::collections::HashSet<i64>> {
        let mut statement = self
            .connection
            .prepare("SELECT id FROM semantic_claims WHERE retracted_by IS NULL")
            .map_err(database_error)?;
        statement
            .query_map([], |row| row.get(0))
            .map_err(database_error)?
            .collect::<Result<std::collections::HashSet<i64>, _>>()
            .map_err(database_error)
    }

    pub fn latest_reply_challenges(
        &self,
        conversation_id: &str,
    ) -> io::Result<Vec<StoredAssistantClaim>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT c.id, c.reply_id, c.subject, c.relation, c.object, c.evidence_text,
                    c.verdict, c.reason, c.evidence_refs, c.confidence_before, c.confidence_after
             FROM assistant_claims c WHERE c.verdict = 'challenged' AND c.reply_id = (
                SELECT id FROM assistant_replies WHERE conversation_id = ?1
                ORDER BY turn_number DESC LIMIT 1
             ) ORDER BY c.id",
            )
            .map_err(database_error)?;
        statement
            .query_map([conversation_id], read_assistant_claim)
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }
    pub fn enqueue_research(
        &self,
        conversation_id: &str,
        turn_number: usize,
        question: &str,
    ) -> io::Result<()> {
        self.connection.execute(
            "INSERT INTO scholar_jobs (id, conversation_id, turn_number, question, status, created_at)
             VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, 'pending',
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(conversation_id, turn_number) DO NOTHING",
            params![conversation_id, i64::try_from(turn_number).map_err(|_| invalid("turn number too large"))?, question],
        ).map_err(database_error)?;
        Ok(())
    }

    /// Atomically record Scout's reason and enqueue the corresponding Scholar job.
    /// One question per topic is enough for this first autonomous research pass.
    pub fn enqueue_scout_research(
        &mut self,
        trigger_kind: &str,
        topic_key: &str,
        trigger_ref: &str,
        question: &str,
    ) -> io::Result<bool> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let recent: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM scout_investigations
             WHERE created_at > strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-5 minutes')",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if recent > 0 {
            return Ok(false);
        }
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM scout_investigations
             WHERE trigger_kind = ?1 AND topic_key = ?2)",
                params![trigger_kind, topic_key],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if exists {
            return Ok(false);
        }
        let job_id: String = tx
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
            .map_err(database_error)?;
        tx.execute(
            "INSERT INTO scholar_jobs
             (id, conversation_id, turn_number, question, status, created_at)
             VALUES (?1, ?2, 0, ?3, 'pending', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![job_id, format!("scout:{job_id}"), question],
        )
        .map_err(database_error)?;
        tx.execute(
            "INSERT INTO scout_investigations
             (trigger_kind, topic_key, trigger_ref, question, scholar_job_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![trigger_kind, topic_key, trigger_ref, question, job_id],
        )
        .map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        Ok(true)
    }

    pub fn scout_investigations(&self) -> io::Result<Vec<ScoutInvestigation>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT trigger_kind, topic_key, trigger_ref, question, scholar_job_id
             FROM scout_investigations ORDER BY created_at DESC LIMIT 30",
            )
            .map_err(database_error)?;
        statement
            .query_map([], |row| {
                Ok(ScoutInvestigation {
                    trigger_kind: row.get(0)?,
                    topic_key: row.get(1)?,
                    trigger_ref: row.get(2)?,
                    question: row.get(3)?,
                    scholar_job_id: row.get(4)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn scout_topic_exists(&self, trigger_kind: &str, topic_key: &str) -> io::Result<bool> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM scout_investigations
             WHERE trigger_kind = ?1 AND topic_key = ?2)",
                params![trigger_kind, topic_key],
                |row| row.get(0),
            )
            .map_err(database_error)
    }

    pub fn approve_model_claim(&self, claim_id: i64, reason: &str) -> io::Result<()> {
        if reason.trim().is_empty()
            || reason.chars().count() > 500
            || reason.chars().any(char::is_control)
        {
            return Err(invalid(
                "model approval needs a printable reason (1 to 500 characters)",
            ));
        }
        let changed = self.connection.execute(
            "INSERT INTO model_claim_approvals (claim_id, reason, approved_at)
             SELECT id, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             FROM semantic_claims WHERE id = ?1 AND retracted_by IS NULL
             ON CONFLICT(claim_id) DO UPDATE SET reason = excluded.reason, approved_at = excluded.approved_at",
            params![claim_id, reason.trim()],
        ).map_err(database_error)?;
        if changed == 0 {
            return Err(invalid("no active semantic claim with that ID"));
        }
        Ok(())
    }

    pub fn approved_model_claim(&self, claim_id: i64) -> io::Result<SemanticClaim> {
        self.connection
            .query_row(
                "SELECT c.id, c.subject, c.relation, c.object, c.source, c.source_observation_id
             FROM semantic_claims c JOIN model_claim_approvals a ON a.claim_id = c.id
             WHERE c.id = ?1 AND c.retracted_by IS NULL",
                [claim_id],
                |row| {
                    Ok(SemanticClaim {
                        id: row.get(0)?,
                        subject: row.get(1)?,
                        relation: row.get(2)?,
                        object: row.get(3)?,
                        source: row.get(4)?,
                        source_observation_id: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("claim is inactive or has not been approved for model training"))
    }

    pub fn record_model_consolidation(
        &self,
        claim_id: i64,
        source: &str,
        output: &str,
        loss: f32,
    ) -> io::Result<()> {
        self.connection
            .execute(
                "INSERT INTO model_consolidations
             (claim_id, source_checkpoint, output_checkpoint, training_loss, created_at)
             VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![claim_id, source, output, loss],
            )
            .map_err(database_error)?;
        Ok(())
    }

    pub fn model_consolidations(&self) -> io::Result<Vec<ModelConsolidation>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, claim_id, source_checkpoint, output_checkpoint, training_loss
             FROM model_consolidations ORDER BY id DESC LIMIT 30",
            )
            .map_err(database_error)?;
        statement
            .query_map([], |row| {
                Ok(ModelConsolidation {
                    id: row.get(0)?,
                    claim_id: row.get(1)?,
                    source_checkpoint: row.get(2)?,
                    output_checkpoint: row.get(3)?,
                    training_loss: row.get(4)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn claim_scholar_job(&mut self) -> io::Result<Option<ScholarJob>> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let job = tx.query_row(
            "SELECT id, question, status FROM scholar_jobs WHERE status = 'pending'
             OR (status = 'processing' AND claimed_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-600 seconds'))
             ORDER BY created_at LIMIT 1",
            [],
            |row| Ok(ScholarJob { id: row.get(0)?, question: row.get(1)?, status: row.get(2)? }),
        ).optional().map_err(database_error)?;
        if let Some(job) = &job {
            tx.execute("UPDATE scholar_jobs SET status = 'processing', claimed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), attempts = attempts + 1 WHERE id = ?1", [&job.id]).map_err(database_error)?;
        }
        tx.commit().map_err(database_error)?;
        Ok(job)
    }

    pub fn finish_scholar_job(
        &mut self,
        job_id: &str,
        source_title: &str,
        source_url: &str,
        source_excerpt: &str,
        finding: Option<ScholarFinding>,
        note: Option<&str>,
    ) -> io::Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let mut job_status = "no_result";
        let mut reason = note.map(str::to_owned);
        if let Some(finding) = finding {
            let safe_field = |value: &str| {
                !value.trim().is_empty()
                    && value.chars().count() <= 500
                    && !value.chars().any(char::is_control)
                    && !value.contains("<|")
            };
            let supported = safe_field(&finding.evidence_text)
                && source_excerpt.contains(&finding.evidence_text)
                && safe_field(&finding.subject)
                && safe_field(&finding.relation)
                && safe_field(&finding.object)
                && (evidence_supports_claim(&finding)
                    || (finding.relation == "has encyclopedia summary"
                        && canonical(&finding.subject) == canonical(source_title)
                        && finding.object == finding.evidence_text))
                && finding.confidence.is_finite()
                && (0.0..=1.0).contains(&finding.confidence);
            let mut semantic_id = None;
            let mut finding_status = "needs_review";
            if !supported {
                reason =
                    Some("scholar claim is not directly supported by its source excerpt".into());
            } else if finding.confidence < PROMOTION_CONFIDENCE {
                reason = Some("scholar extraction confidence below promotion threshold".into());
            } else if matches!(canonical(&finding.subject).as_str(), "user" | "the user") {
                reason =
                    Some("web research cannot establish a personal fact about the user".into());
            } else {
                let existing: Option<(i64, String)> = tx
                    .query_row(
                        "SELECT id, object FROM semantic_claims WHERE retracted_by IS NULL
                     AND lower(trim(subject)) = lower(trim(?1))
                     AND lower(trim(relation)) = lower(trim(?2)) ORDER BY id DESC LIMIT 1",
                        params![finding.subject, finding.relation],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(database_error)?;
                match existing {
                    Some((id, object)) if canonical(&object) == canonical(&finding.object) => {
                        semantic_id = Some(id);
                        finding_status = "promoted";
                    }
                    Some((id, _)) => reason = Some(format!("conflicts with semantic claim #{id}")),
                    None => {
                        tx.execute(
                            "INSERT INTO semantic_claims (subject, relation, object, source, source_observation_id, created_at)
                             VALUES (?1, ?2, ?3, ?4, NULL, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                            params![finding.subject.trim(), finding.relation.trim(), finding.object.trim(), source_url],
                        ).map_err(database_error)?;
                        semantic_id = Some(tx.last_insert_rowid());
                        finding_status = "promoted";
                    }
                }
            }
            tx.execute(
                "INSERT INTO scholar_findings (job_id, source_title, source_url, source_excerpt,
                  subject, relation, object, evidence_text, confidence, status, semantic_claim_id, decision_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![job_id, source_title, source_url, source_excerpt, finding.subject, finding.relation,
                    finding.object, finding.evidence_text, finding.confidence, finding_status, semantic_id, reason],
            ).map_err(database_error)?;
            job_status = finding_status;
        }
        let changed = tx
            .execute(
                "UPDATE scholar_jobs SET status = ?2, last_error = ?3, claimed_at = NULL
             WHERE id = ?1 AND status = 'processing'",
                params![job_id, job_status, reason],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("scholar job is not processing"));
        }
        tx.commit().map_err(database_error)
    }

    pub fn hold_scholar_job(&self, job_id: &str, reason: &str) -> io::Result<()> {
        self.connection.execute(
            "UPDATE scholar_jobs SET status = 'needs_review', last_error = ?2, claimed_at = NULL
             WHERE id = ?1 AND status = 'processing'",
            params![job_id, reason],
        ).map_err(database_error)?;
        Ok(())
    }

    pub fn scholar_jobs(&self) -> io::Result<Vec<(ScholarJob, Option<String>)>> {
        let mut statement = self.connection.prepare(
            "SELECT id, question, status, last_error FROM scholar_jobs ORDER BY created_at DESC LIMIT 30"
        ).map_err(database_error)?;
        statement
            .query_map([], |row| {
                Ok((
                    ScholarJob {
                        id: row.get(0)?,
                        question: row.get(1)?,
                        status: row.get(2)?,
                    },
                    row.get(3)?,
                ))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn scholar_findings(&self, job_id: &str) -> io::Result<Vec<StoredScholarFinding>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, source_url, subject, relation, object, evidence_text,
                    confidence, status, decision_reason
             FROM scholar_findings WHERE job_id = ?1 ORDER BY id",
            )
            .map_err(database_error)?;
        statement
            .query_map([job_id], |row| {
                Ok(StoredScholarFinding {
                    id: row.get(0)?,
                    source_url: row.get(1)?,
                    subject: row.get(2)?,
                    relation: row.get(3)?,
                    object: row.get(4)?,
                    evidence_text: row.get(5)?,
                    confidence: row.get(6)?,
                    status: row.get(7)?,
                    decision_reason: row.get(8)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn review_scholar_finding(
        &mut self,
        finding_id: i64,
        accept: bool,
        reason: &str,
    ) -> io::Result<()> {
        if reason.trim().is_empty()
            || reason.chars().count() > 500
            || reason.chars().any(char::is_control)
        {
            return Err(invalid(
                "review reason must contain 1 to 500 printable characters",
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let finding = tx
            .query_row(
                "SELECT job_id, source_url, subject, relation, object, status
             FROM scholar_findings WHERE id = ?1",
                [finding_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("no scholar finding with that ID"))?;
        let (job_id, source_url, subject, relation, object, status) = finding;
        if status != "needs_review" {
            return Err(invalid("scholar finding is already resolved"));
        }
        let semantic_id = if accept {
            let mut statement = tx.prepare(
                "SELECT id, subject, relation, object FROM semantic_claims WHERE retracted_by IS NULL"
            ).map_err(database_error)?;
            let current = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?;
            drop(statement);
            let equivalent = current
                .iter()
                .find(|(_, s, r, o)| {
                    canonical(s) == canonical(&subject)
                        && canonical(r) == canonical(&relation)
                        && canonical(o) == canonical(&object)
                })
                .map(|(id, _, _, _)| *id);
            if let Some(id) = equivalent {
                Some(id)
            } else {
                let conflicting = current
                    .iter()
                    .find(|(_, s, r, _)| {
                        canonical(s) == canonical(&subject) && canonical(r) == canonical(&relation)
                    })
                    .map(|(id, _, _, _)| *id);
                if let Some(id) = conflicting {
                    tx.execute(
                        "UPDATE semantic_claims SET retracted_by = ?2 WHERE id = ?1",
                        params![
                            id,
                            format!(
                                "superseded by reviewed scholar finding #{finding_id}: {}",
                                reason.trim()
                            )
                        ],
                    )
                    .map_err(database_error)?;
                }
                tx.execute(
                    "INSERT INTO semantic_claims (subject, relation, object, source, supersedes_id, source_observation_id, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, NULL, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                    params![subject, relation, object, source_url, conflicting],
                ).map_err(database_error)?;
                Some(tx.last_insert_rowid())
            }
        } else {
            None
        };
        tx.execute(
            "UPDATE scholar_findings SET status = ?2, semantic_claim_id = ?3, decision_reason = ?4 WHERE id = ?1",
            params![finding_id, if accept { "promoted" } else { "rejected" }, semantic_id, reason.trim()],
        ).map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_jobs SET status = ?2, last_error = NULL WHERE id = ?1",
            params![job_id, if accept { "promoted" } else { "no_result" }],
        )
        .map_err(database_error)?;
        tx.commit().map_err(database_error)
    }
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

    /// Import the JSON map without changing it. Re-running is safe: matching
    /// rows are accepted, while an ID with different content aborts the import.
    pub fn import_knowledge(&mut self, map: &KnowledgeMap) -> io::Result<ImportSummary> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let mut inserted = 0;
        let mut already_present = 0;

        for claim in map.entries() {
            match get_claim(&tx, claim.id)? {
                Some(existing) if claims_equal(&existing, claim) => already_present += 1,
                Some(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "SQLite already has claim #{} with different content; import stopped",
                            claim.id
                        ),
                    ));
                }
                None => {
                    let id = i64::try_from(claim.id)
                        .map_err(|_| invalid("claim ID exceeds SQLite's integer range"))?;
                    let supersedes =
                        claim
                            .supersedes
                            .map(i64::try_from)
                            .transpose()
                            .map_err(|_| {
                                invalid("superseded claim ID exceeds SQLite's integer range")
                            })?;
                    tx.execute(
                        "INSERT INTO semantic_claims
                            (id, subject, relation, object, source, supersedes_id,
                             retracted_by, source_observation_id, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)",
                        params![
                            id,
                            claim.subject,
                            claim.relation,
                            claim.object,
                            claim.source,
                            supersedes,
                            claim.retracted_by
                        ],
                    )
                    .map_err(database_error)?;
                    inserted += 1;
                }
            }
        }

        tx.commit().map_err(database_error)?;
        Ok(ImportSummary {
            inserted,
            already_present,
        })
    }

    /// Persist one user turn before generation. Retrying the same turn is
    /// idempotent and returns its original observation ID.
    pub fn stage_observation(
        &self,
        conversation_id: &str,
        turn_number: usize,
        raw_text: &str,
    ) -> io::Result<String> {
        let turn_number = i64::try_from(turn_number)
            .map_err(|_| invalid("conversation turn exceeds SQLite's integer range"))?;
        self.connection
            .execute(
                "INSERT INTO observations
                    (id, conversation_id, turn_number, speaker, raw_text,
                     created_at, status)
                 VALUES (lower(hex(randomblob(16))), ?1, ?2, 'user', ?3,
                         strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'pending')
                 ON CONFLICT(conversation_id, turn_number, raw_text) DO NOTHING",
                params![conversation_id, turn_number, raw_text],
            )
            .map_err(database_error)?;
        self.connection
            .query_row(
                "SELECT id FROM observations
                 WHERE conversation_id = ?1 AND turn_number = ?2 AND raw_text = ?3",
                params![conversation_id, turn_number, raw_text],
                |row| row.get(0),
            )
            .map_err(database_error)
    }

    /// Find recent unprocessed observations from this conversation that share
    /// meaningful words with the current question.
    pub fn relevant_observations(
        &self,
        conversation_id: &str,
        query: &str,
        exclude_id: &str,
        limit: usize,
    ) -> io::Result<Vec<Observation>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let query_words = relevant_words(query);
        if query_words.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT o.id, o.turn_number, o.raw_text, o.status, o.created_at,
                        COALESCE((SELECT group_concat(c.subject || ' ' || c.relation || ' ' || c.object, ' ')
                                  FROM candidate_claims c
                                  WHERE c.observation_id = o.id
                                    AND c.status IN ('proposed', 'needs_review')
                                    AND lower(trim(c.subject)) IN ('user', 'the user')), '')
                 FROM observations o
                 WHERE o.conversation_id = ?1
                   AND o.id != ?2
                   AND o.status IN ('pending', 'processing', 'needs_review')
                 ORDER BY o.turn_number DESC LIMIT 256",
            )
            .map_err(database_error)?;
        let candidates = statement
            .query_map(params![conversation_id, exclude_id], |row| {
                let turn_number: i64 = row.get(1)?;
                Ok((
                    Observation {
                        id: row.get(0)?,
                        conversation_id: conversation_id.to_owned(),
                        turn_number: usize::try_from(turn_number).map_err(|_| {
                            rusqlite::Error::IntegralValueOutOfRange(1, turn_number)
                        })?,
                        raw_text: row.get(2)?,
                        status: row.get(3)?,
                        created_at: row.get(4)?,
                    },
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(database_error)?;
        let mut scored = Vec::new();
        for result in candidates {
            let (observation, candidate_text) = result.map_err(database_error)?;
            let score = lexical_overlap(&relevant_words(&observation.raw_text), &query_words).max(
                lexical_overlap(&relevant_words(&candidate_text), &query_words),
            );
            if score > 0 {
                scored.push((score, observation));
            }
        }
        scored.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| right.turn_number.cmp(&left.turn_number))
        });
        Ok(scored
            .into_iter()
            .take(limit)
            .map(|(_, observation)| observation)
            .collect())
    }

    /// Claim one queued observation. A worker that disappears leaves a lease
    /// which becomes available again after `lease_seconds`.
    pub fn claim_next_observation(
        &mut self,
        lease_seconds: u64,
    ) -> io::Result<Option<Observation>> {
        let lease_modifier = format!("-{} seconds", lease_seconds);
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let candidate = tx
            .query_row(
                "SELECT id, conversation_id, turn_number, raw_text, status, created_at
                 FROM observations
                 WHERE status = 'pending'
                    OR (status = 'processing' AND
                        (claimed_at IS NULL OR claimed_at <
                         strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?1)))
                 ORDER BY created_at, turn_number
                 LIMIT 1",
                [&lease_modifier],
                |row| {
                    let turn_number: i64 = row.get(2)?;
                    Ok(Observation {
                        id: row.get(0)?,
                        conversation_id: row.get(1)?,
                        turn_number: usize::try_from(turn_number).map_err(|_| {
                            rusqlite::Error::IntegralValueOutOfRange(2, turn_number)
                        })?,
                        raw_text: row.get(3)?,
                        status: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(database_error)?;
        if let Some(observation) = candidate {
            tx.execute(
                "UPDATE observations
                 SET status = 'processing',
                     claimed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                     retry_count = retry_count + 1
                 WHERE id = ?1",
                [&observation.id],
            )
            .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
            Ok(Some(Observation {
                status: "processing".into(),
                ..observation
            }))
        } else {
            tx.commit().map_err(database_error)?;
            Ok(None)
        }
    }

    /// Curate extracted candidates, retain provenance, and finish the
    /// observation atomically.
    pub fn finish_extraction(
        &mut self,
        observation_id: &str,
        claims: &[CandidateClaim],
        review_reason: Option<&str>,
    ) -> io::Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let mut review_reasons = Vec::new();
        for claim in claims {
            let mut claim = claim.clone();
            claim.subject = normalize(&claim.subject);
            claim.relation = normalize(&claim.relation);
            claim.object = normalize(&claim.object);
            let semantic_id =
                if claim.status == "proposed" && claim.confidence >= PROMOTION_CONFIDENCE {
                    match curate_candidate(&tx, observation_id, &mut claim)? {
                        Ok(id) => Some(id),
                        Err(reason) => {
                            claim.status = "needs_review".into();
                            claim.decision_reason = Some(reason.clone());
                            review_reasons.push(reason);
                            None
                        }
                    }
                } else {
                    claim.status = "needs_review".into();
                    claim.decision_reason.get_or_insert_with(|| {
                        format!("confidence below promotion threshold of {PROMOTION_CONFIDENCE:.2}")
                    });
                    review_reasons.push(claim.decision_reason.clone().unwrap());
                    None
                };
            let candidate_id: String = tx
                .query_row(
                    "INSERT INTO candidate_claims
                    (id, observation_id, subject, relation, object, evidence_text,
                     context_message_ids, resolution, confidence, status, decision_reason,
                     semantic_claim_id)
                 VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, ?4, ?5, ?6,
                         ?7, ?8, ?9, ?10, ?11)
                 RETURNING id",
                    params![
                        observation_id,
                        claim.subject,
                        claim.relation,
                        claim.object,
                        claim.evidence_text,
                        serde_json::to_string(&claim.context_message_ids).map_err(invalid_json)?,
                        claim.resolution,
                        claim.confidence,
                        claim.status,
                        claim.decision_reason,
                        semantic_id
                    ],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            if claim.status == "promoted" {
                attach_equivalent_sources(
                    &tx,
                    semantic_id.unwrap(),
                    &candidate_id,
                    &claim.subject,
                    &claim.relation,
                    &claim.object,
                )?;
            }
        }
        if let Some(reason) = review_reason {
            review_reasons.push(reason.to_owned());
        }
        let review_summary = (!review_reasons.is_empty()).then(|| {
            review_reasons
                .join("; ")
                .chars()
                .take(500)
                .collect::<String>()
        });
        let (status, reason) = if let Some(reason) = review_summary.as_deref() {
            ("needs_review", Some(reason))
        } else if claims.is_empty() {
            ("rejected", Some("no supported claims extracted"))
        } else {
            ("promoted", None)
        };
        let changed = tx
            .execute(
                "UPDATE observations
                 SET status = ?2, last_error = ?3, claimed_at = NULL
                 WHERE id = ?1 AND status = 'processing'",
                params![observation_id, status, reason],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("observation is not currently claimed by a worker"));
        }
        tx.commit().map_err(database_error)
    }

    pub fn hold_observation(&mut self, observation_id: &str, reason: &str) -> io::Result<()> {
        self.connection
            .execute(
                "UPDATE observations
                 SET status = 'needs_review', last_error = ?2, claimed_at = NULL
                 WHERE id = ?1 AND status = 'processing'",
                params![observation_id, reason],
            )
            .map_err(database_error)?;
        Ok(())
    }

    pub fn claim_count(&self) -> io::Result<usize> {
        let count: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM semantic_claims", [], |row| row.get(0))
            .map_err(database_error)?;
        usize::try_from(count).map_err(|_| invalid("SQLite returned a negative claim count"))
    }

    pub fn relevant_semantic_claims(
        &self,
        query: &str,
        limit: usize,
    ) -> io::Result<Vec<SemanticClaim>> {
        let query_words = relevant_words(query);
        if query_words.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, subject, relation, object, source, source_observation_id
             FROM semantic_claims WHERE retracted_by IS NULL ORDER BY id DESC LIMIT 1000",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(SemanticClaim {
                    id: row.get(0)?,
                    subject: row.get(1)?,
                    relation: row.get(2)?,
                    object: row.get(3)?,
                    source: row.get(4)?,
                    source_observation_id: row.get(5)?,
                })
            })
            .map_err(database_error)?;
        let mut ranked = Vec::new();
        for row in rows {
            let claim = row.map_err(database_error)?;
            let words = relevant_words(&format!(
                "{} {} {}",
                claim.subject, claim.relation, claim.object
            ));
            let score = lexical_overlap(&words, &query_words);
            if score > 0 {
                ranked.push((score, claim));
            }
        }
        ranked.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(ranked
            .into_iter()
            .take(limit)
            .map(|(_, claim)| claim)
            .collect())
    }

    pub fn inbox_summary(&self) -> io::Result<InboxSummary> {
        let counts = ["pending", "processing", "needs_review"]
            .into_iter()
            .map(|status| {
                self.connection
                    .query_row(
                        "SELECT COUNT(*) FROM observations WHERE status = ?1",
                        [status],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(database_error)
                    .and_then(|count| {
                        usize::try_from(count).map_err(|_| invalid("negative inbox count"))
                    })
            })
            .collect::<io::Result<Vec<_>>>()?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, conversation_id, turn_number, raw_text, status, created_at
             FROM observations WHERE status IN ('pending', 'processing', 'needs_review')
             ORDER BY created_at, turn_number",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| {
                let turn: i64 = row.get(2)?;
                Ok(Observation {
                    id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    turn_number: usize::try_from(turn)
                        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(2, turn))?,
                    raw_text: row.get(3)?,
                    status: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })
            .map_err(database_error)?;
        let observations = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        Ok(InboxSummary {
            pending: counts[0],
            processing: counts[1],
            needs_review: counts[2],
            observations,
        })
    }

    pub fn observation_detail(&self, id: &str) -> io::Result<Option<ObservationDetail>> {
        let observation = self
            .connection
            .query_row(
                "SELECT id, conversation_id, turn_number, raw_text, status, created_at, last_error
             FROM observations WHERE id = ?1",
                [id],
                |row| {
                    let turn: i64 = row.get(2)?;
                    Ok((
                        Observation {
                            id: row.get(0)?,
                            conversation_id: row.get(1)?,
                            turn_number: usize::try_from(turn)
                                .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(2, turn))?,
                            raw_text: row.get(3)?,
                            status: row.get(4)?,
                            created_at: row.get(5)?,
                        },
                        row.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?;
        let Some((observation, last_error)) = observation else {
            return Ok(None);
        };
        let mut statement = self
            .connection
            .prepare(
            "SELECT id, observation_id, subject, relation, object, evidence_text, confidence, status,
                    decision_reason, semantic_claim_id
             FROM candidate_claims WHERE observation_id = ?1 ORDER BY rowid",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([id], |row| {
                Ok(StoredCandidate {
                    id: row.get(0)?,
                    observation_id: row.get(1)?,
                    subject: row.get(2)?,
                    relation: row.get(3)?,
                    object: row.get(4)?,
                    evidence_text: row.get(5)?,
                    confidence: row.get(6)?,
                    status: row.get(7)?,
                    decision_reason: row.get(8)?,
                    semantic_claim_id: row.get(9)?,
                })
            })
            .map_err(database_error)?;
        let candidates = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        Ok(Some(ObservationDetail {
            observation,
            last_error,
            candidates,
        }))
    }

    pub fn observation_text(&self, id: &str) -> io::Result<Option<String>> {
        self.connection
            .query_row(
                "SELECT raw_text FROM observations WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)
    }

    pub fn relevant_review_candidates(
        &self,
        conversation_id: &str,
        query: &str,
        limit: usize,
    ) -> io::Result<Vec<StoredCandidate>> {
        let query_words = relevant_words(query);
        if query_words.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT c.id, c.observation_id, c.subject, c.relation, c.object,
                    c.evidence_text, c.confidence, c.status, c.decision_reason,
                    c.semantic_claim_id, o.turn_number
             FROM candidate_claims c JOIN observations o ON o.id = c.observation_id
             WHERE o.conversation_id = ?1 AND c.status IN ('proposed', 'needs_review')
             ORDER BY o.turn_number DESC LIMIT 256",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([conversation_id], |row| {
                Ok((
                    StoredCandidate {
                        id: row.get(0)?,
                        observation_id: row.get(1)?,
                        subject: row.get(2)?,
                        relation: row.get(3)?,
                        object: row.get(4)?,
                        evidence_text: row.get(5)?,
                        confidence: row.get(6)?,
                        status: row.get(7)?,
                        decision_reason: row.get(8)?,
                        semantic_claim_id: row.get(9)?,
                    },
                    row.get::<_, i64>(10)?,
                ))
            })
            .map_err(database_error)?;
        let mut ranked = Vec::new();
        for row in rows {
            let (candidate, turn) = row.map_err(database_error)?;
            let words = relevant_words(&format!(
                "{} {} {}",
                candidate.subject, candidate.relation, candidate.object
            ));
            let score = lexical_overlap(&words, &query_words);
            if score > 0 {
                ranked.push((score, turn, candidate));
            }
        }
        ranked.sort_by(|(left_score, left_turn, _), (right_score, right_turn, _)| {
            right_score
                .cmp(left_score)
                .then_with(|| right_turn.cmp(left_turn))
        });
        Ok(ranked
            .into_iter()
            .take(limit)
            .map(|(_, _, candidate)| candidate)
            .collect())
    }

    pub fn active_semantic_claims(&self) -> io::Result<Vec<SemanticClaimDetail>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, subject, relation, object, source, source_observation_id
             FROM semantic_claims WHERE retracted_by IS NULL ORDER BY id",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(SemanticClaim {
                    id: row.get(0)?,
                    subject: row.get(1)?,
                    relation: row.get(2)?,
                    object: row.get(3)?,
                    source: row.get(4)?,
                    source_observation_id: row.get(5)?,
                })
            })
            .map_err(database_error)?;
        let claims = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        drop(statement);
        claims
            .into_iter()
            .map(|claim| {
                let mut sources_statement = self
                    .connection
                    .prepare(
                        "SELECT o.id || ': ' || o.raw_text FROM semantic_claim_sources s
                 JOIN observations o ON o.id = s.observation_id
                 WHERE s.semantic_claim_id = ?1 ORDER BY o.created_at, o.turn_number",
                    )
                    .map_err(database_error)?;
                let sources = sources_statement
                    .query_map([claim.id], |row| row.get(0))
                    .map_err(database_error)?
                    .collect::<Result<Vec<String>, _>>()
                    .map_err(database_error)?;
                let mut sources: Vec<String> = sources;
                let mut scholar_statement = self
                    .connection
                    .prepare(
                        "SELECT source_url || ': ' || evidence_text FROM scholar_findings
                     WHERE semantic_claim_id = ?1 AND status = 'promoted' ORDER BY id",
                    )
                    .map_err(database_error)?;
                let scholar_sources = scholar_statement
                    .query_map([claim.id], |row| row.get(0))
                    .map_err(database_error)?
                    .collect::<Result<Vec<String>, _>>()
                    .map_err(database_error)?;
                sources.extend(scholar_sources);
                Ok(SemanticClaimDetail { claim, sources })
            })
            .collect()
    }

    pub fn resolve_candidate(&mut self, id: &str, accept: bool, reason: &str) -> io::Result<()> {
        if reason.trim().is_empty()
            || reason.chars().count() > 500
            || reason.chars().any(char::is_control)
        {
            return Err(invalid(
                "review reason must contain 1 to 500 printable characters",
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let candidate = tx
            .query_row(
                "SELECT id, observation_id, subject, relation, object, evidence_text,
                    context_message_ids, resolution, confidence, status
             FROM candidate_claims WHERE id = ?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, f64>(8)?,
                        row.get::<_, String>(9)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("no candidate with that ID"))?;
        let (
            candidate_id,
            observation_id,
            subject,
            relation,
            object,
            evidence,
            context,
            resolution,
            confidence,
            status,
        ) = candidate;
        if status != "needs_review" && status != "proposed" {
            return Err(invalid("candidate is already resolved"));
        }
        let semantic_id = if accept {
            let mut claim = CandidateClaim {
                subject: normalize(&subject),
                relation: normalize(&relation),
                object: normalize(&object),
                evidence_text: evidence,
                context_message_ids: serde_json::from_str(&context).map_err(invalid_json)?,
                resolution,
                confidence,
                status: "proposed".into(),
                decision_reason: None,
            };
            let semantic_id = match curate_candidate(&tx, &observation_id, &mut claim)? {
                Ok(id) => id,
                Err(conflict) => {
                    let mut statement = tx.prepare(
                        "SELECT id, subject, relation FROM semantic_claims WHERE retracted_by IS NULL",
                    ).map_err(database_error)?;
                    let rows = statement
                        .query_map([], |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                            ))
                        })
                        .map_err(database_error)?;
                    let mut current_id = None;
                    for row in rows {
                        let (id, subject, relation) = row.map_err(database_error)?;
                        if canonical(&subject) == canonical(&claim.subject)
                            && canonical(&relation) == canonical(&claim.relation)
                        {
                            current_id = Some(id);
                            break;
                        }
                    }
                    drop(statement);
                    let current_id = current_id.ok_or_else(|| invalid(&conflict))?;
                    tx.execute(
                        "UPDATE semantic_claims SET retracted_by = ?2 WHERE id = ?1",
                        params![
                            current_id,
                            format!(
                                "superseded by reviewed candidate {candidate_id}: {}",
                                reason.trim()
                            )
                        ],
                    )
                    .map_err(database_error)?;
                    tx.execute(
                        "INSERT INTO semantic_claims
                            (subject, relation, object, source, supersedes_id, source_observation_id, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                        params![claim.subject, claim.relation, claim.object, format!("reviewed candidate {candidate_id}: {}", reason.trim()), current_id, observation_id],
                    ).map_err(database_error)?;
                    tx.query_row("SELECT last_insert_rowid()", [], |row| row.get(0))
                        .map_err(database_error)?
                }
            };
            tx.execute("INSERT OR IGNORE INTO semantic_claim_sources (semantic_claim_id, candidate_id, observation_id) VALUES (?1, ?2, ?3)", params![semantic_id, candidate_id, observation_id]).map_err(database_error)?;
            Some(semantic_id)
        } else {
            None
        };
        tx.execute(
            "UPDATE candidate_claims SET status = ?2, decision_reason = ?3, semantic_claim_id = ?4 WHERE id = ?1",
            params![candidate_id, if accept { "promoted" } else { "rejected" }, reason.trim(), semantic_id],
        ).map_err(database_error)?;
        let remaining: i64 = tx.query_row(
            "SELECT COUNT(*) FROM candidate_claims WHERE observation_id = ?1 AND status IN ('proposed', 'needs_review')",
            [&observation_id], |row| row.get(0),
        ).map_err(database_error)?;
        if remaining == 0 {
            let promoted: i64 = tx.query_row(
                "SELECT COUNT(*) FROM candidate_claims WHERE observation_id = ?1 AND status = 'promoted'",
                [&observation_id], |row| row.get(0),
            ).map_err(database_error)?;
            tx.execute(
                "UPDATE observations SET status = ?2, last_error = NULL WHERE id = ?1",
                params![
                    observation_id,
                    if promoted > 0 { "promoted" } else { "rejected" }
                ],
            )
            .map_err(database_error)?;
        }
        tx.commit().map_err(database_error)
    }

    fn migrate(&mut self) -> io::Result<()> {
        let version: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(database_error)?;
        if version > SCHEMA_VERSION {
            return Err(invalid("learning database was created by a newer version"));
        }
        if version == SCHEMA_VERSION {
            return Ok(());
        }
        if version < 1 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            create_v1(&tx)?;
            tx.pragma_update(None, "user_version", 1)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 2 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE UNIQUE INDEX observations_are_idempotent
                 ON observations(conversation_id, turn_number, raw_text);",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 2)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 3 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch("ALTER TABLE observations ADD COLUMN claimed_at TEXT;")
                .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 3)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 4 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch("ALTER TABLE candidate_claims ADD COLUMN semantic_claim_id INTEGER REFERENCES semantic_claims(id);
                CREATE TABLE semantic_claim_sources (
                    semantic_claim_id INTEGER NOT NULL REFERENCES semantic_claims(id),
                    candidate_id TEXT NOT NULL REFERENCES candidate_claims(id),
                    observation_id TEXT NOT NULL REFERENCES observations(id),
                    PRIMARY KEY (semantic_claim_id, observation_id)
                );")
                .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 4)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 5 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE scholar_jobs (
                    id TEXT PRIMARY KEY,
                    conversation_id TEXT NOT NULL,
                    turn_number INTEGER NOT NULL,
                    question TEXT NOT NULL,
                    status TEXT NOT NULL CHECK (status IN ('pending', 'processing', 'promoted', 'needs_review', 'no_result')),
                    created_at TEXT NOT NULL,
                    claimed_at TEXT,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT,
                    UNIQUE(conversation_id, turn_number)
                );
                CREATE INDEX scholar_jobs_by_status ON scholar_jobs(status, created_at);
                CREATE TABLE scholar_findings (
                    id INTEGER PRIMARY KEY,
                    job_id TEXT NOT NULL REFERENCES scholar_jobs(id),
                    source_title TEXT NOT NULL,
                    source_url TEXT NOT NULL,
                    source_excerpt TEXT NOT NULL,
                    subject TEXT NOT NULL,
                    relation TEXT NOT NULL,
                    object TEXT NOT NULL,
                    evidence_text TEXT NOT NULL,
                    confidence REAL NOT NULL,
                    status TEXT NOT NULL CHECK (status IN ('promoted', 'needs_review', 'rejected')),
                    semantic_claim_id INTEGER REFERENCES semantic_claims(id),
                    decision_reason TEXT
                );"
            ).map_err(database_error)?;
            tx.pragma_update(None, "user_version", 5)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 6 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE assistant_replies (
                    id TEXT PRIMARY KEY,
                    conversation_id TEXT NOT NULL,
                    turn_number INTEGER NOT NULL,
                    reply_text TEXT NOT NULL,
                    status TEXT NOT NULL CHECK (status IN
                        ('pending', 'processing', 'challenged', 'inconclusive', 'no_claims', 'needs_review')),
                    created_at TEXT NOT NULL,
                    claimed_at TEXT,
                    last_error TEXT,
                    UNIQUE(conversation_id, turn_number)
                );
                CREATE INDEX assistant_replies_by_status ON assistant_replies(status, created_at);
                CREATE TABLE assistant_claims (
                    id INTEGER PRIMARY KEY,
                    reply_id TEXT NOT NULL REFERENCES assistant_replies(id),
                    subject TEXT NOT NULL,
                    relation TEXT NOT NULL,
                    object TEXT NOT NULL,
                    evidence_text TEXT NOT NULL,
                    extraction_confidence REAL NOT NULL,
                    confidence_before REAL NOT NULL,
                    confidence_after REAL NOT NULL,
                    verdict TEXT NOT NULL CHECK (verdict IN ('challenged', 'inconclusive')),
                    challenge_type TEXT,
                    reason TEXT NOT NULL,
                    evidence_refs TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );
                CREATE INDEX assistant_claims_by_reply ON assistant_claims(reply_id);"
            ).map_err(database_error)?;
            tx.pragma_update(None, "user_version", 6)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 7 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE scout_investigations (
                    trigger_kind TEXT NOT NULL CHECK (trigger_kind IN ('challenged', 'uncertain', 'sparse')),
                    topic_key TEXT NOT NULL,
                    trigger_ref TEXT NOT NULL,
                    question TEXT NOT NULL,
                    scholar_job_id TEXT NOT NULL REFERENCES scholar_jobs(id),
                    created_at TEXT NOT NULL,
                    PRIMARY KEY (trigger_kind, topic_key)
                );"
            ).map_err(database_error)?;
            tx.pragma_update(None, "user_version", 7)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 8 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE model_claim_approvals (
                    claim_id INTEGER PRIMARY KEY REFERENCES semantic_claims(id),
                    reason TEXT NOT NULL,
                    approved_at TEXT NOT NULL
                );
                CREATE TABLE model_consolidations (
                    id INTEGER PRIMARY KEY,
                    claim_id INTEGER NOT NULL REFERENCES semantic_claims(id),
                    source_checkpoint TEXT NOT NULL,
                    output_checkpoint TEXT NOT NULL,
                    training_loss REAL NOT NULL,
                    created_at TEXT NOT NULL
                );",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 8)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ImportSummary {
    pub inserted: usize,
    pub already_present: usize,
}

fn create_v1(tx: &Transaction<'_>) -> io::Result<()> {
    tx.execute_batch(
        "CREATE TABLE observations (
             id TEXT PRIMARY KEY,
             conversation_id TEXT NOT NULL,
             turn_number INTEGER NOT NULL CHECK (turn_number >= 0),
             speaker TEXT NOT NULL CHECK (speaker = 'user'),
             raw_text TEXT NOT NULL,
             created_at TEXT NOT NULL,
             status TEXT NOT NULL CHECK (status IN
                 ('pending', 'processing', 'promoted', 'needs_review', 'rejected')),
             retry_count INTEGER NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
             last_error TEXT
         );

         CREATE TABLE candidate_claims (
             id TEXT PRIMARY KEY,
             observation_id TEXT NOT NULL REFERENCES observations(id),
             subject TEXT NOT NULL,
             relation TEXT NOT NULL,
             object TEXT NOT NULL,
             evidence_text TEXT NOT NULL,
             context_message_ids TEXT NOT NULL DEFAULT '[]',
             resolution TEXT,
             confidence REAL CHECK (confidence IS NULL OR
                 (confidence >= 0.0 AND confidence <= 1.0)),
             status TEXT NOT NULL CHECK (status IN
                 ('proposed', 'promoted', 'needs_review', 'rejected')),
             decision_reason TEXT
         );

         CREATE TABLE semantic_claims (
             id INTEGER PRIMARY KEY,
             subject TEXT NOT NULL,
             relation TEXT NOT NULL,
             object TEXT NOT NULL,
             source TEXT NOT NULL,
             supersedes_id INTEGER REFERENCES semantic_claims(id),
             retracted_by TEXT,
             source_observation_id TEXT REFERENCES observations(id),
             created_at TEXT
         );

         CREATE INDEX observations_by_status ON observations(status, created_at);
         CREATE INDEX candidate_claims_by_observation
             ON candidate_claims(observation_id);
         CREATE INDEX semantic_claims_by_active
             ON semantic_claims(retracted_by, id);",
    )
    .map_err(database_error)
}

fn curate_candidate(
    tx: &Transaction<'_>,
    observation_id: &str,
    candidate: &mut CandidateClaim,
) -> io::Result<Result<i64, String>> {
    let mut statement = tx
        .prepare(
            "SELECT id, subject, relation, object FROM semantic_claims
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
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(statement);
    for (id, subject, relation, object) in existing {
        if canonical(&subject) == canonical(&candidate.subject)
            && canonical(&relation) == canonical(&candidate.relation)
            && canonical(&object) == canonical(&candidate.object)
        {
            candidate.status = "promoted".into();
            candidate.decision_reason = Some("equivalent active claim; source attached".into());
            return Ok(Ok(id));
        }
        if canonical(&subject) == canonical(&candidate.subject)
            && canonical(&relation) == canonical(&candidate.relation)
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
             source_observation_id, created_at)
         VALUES (?1, ?2, ?3, ?4, NULL, NULL, ?5,
                 strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        params![
            candidate.subject,
            candidate.relation,
            candidate.object,
            format!("user observation {observation_id}"),
            observation_id
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
    subject: &str,
    relation: &str,
    object: &str,
) -> io::Result<()> {
    let mut statement = tx
        .prepare(
            "SELECT id, observation_id FROM candidate_claims
             WHERE lower(trim(subject)) = lower(?1)
               AND lower(trim(relation)) = lower(?2)
               AND lower(trim(object)) = lower(?3)
               AND status IN ('proposed', 'needs_review', 'promoted')",
        )
        .map_err(database_error)?;
    let candidates = statement
        .query_map(params![subject, relation, object], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
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

fn evidence_supports_claim(finding: &ScholarFinding) -> bool {
    let evidence = canonical(&finding.evidence_text);
    if !evidence.contains(&canonical(&finding.subject))
        || !evidence.contains(&canonical(&finding.object))
    {
        return false;
    }
    let relation = canonical(&finding.relation);
    let relation_terms = relation
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| word.len() >= 4)
        .filter(|word| !matches!(*word, "from" | "into" | "that" | "with"));
    relation_terms
        .into_iter()
        .any(|word| evidence.contains(word))
        || relation.split_whitespace().all(|word| word.len() < 4)
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

fn invalid_json(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn relevant_words(text: &str) -> std::collections::BTreeSet<String> {
    const STOP_WORDS: &[&str] = &[
        "a", "an", "and", "are", "did", "do", "does", "for", "from", "how", "i", "in", "is", "it",
        "me", "my", "of", "on", "or", "please", "tell", "that", "the", "to", "was", "what", "when",
        "where", "who", "why", "with", "you", "your",
    ];
    text.to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| word.chars().count() >= 2 && !STOP_WORDS.contains(word))
        .map(str::to_owned)
        .collect()
}

fn lexical_overlap(
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
mod tests {
    use super::{
        AssistantClaim, CandidateClaim, ClaimJudgment, ImportSummary, LearningStore,
        ScholarFinding, evidence_supports_claim,
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

    #[test]
    fn assistant_challenge_is_durable_and_policy_reduces_usable_confidence() {
        let path = temp_path();
        let mut store = LearningStore::open(&path).unwrap();
        store
            .stage_assistant_reply("conversation", 1, "A cricket bat is 400m long.")
            .unwrap();
        store
            .stage_assistant_reply("conversation", 1, "A cricket bat is 400m long.")
            .unwrap();
        let job = store.claim_assistant_reply().unwrap().unwrap();
        assert!(store.claim_assistant_reply().unwrap().is_none());
        store
            .finish_assistant_reply(
                &job.id,
                &[ClaimJudgment {
                    claim: AssistantClaim {
                        subject: "cricket bat".into(),
                        relation: "length".into(),
                        object: "400m".into(),
                        evidence_text: "A cricket bat is 400m long.".into(),
                        extraction_confidence: 0.8,
                    },
                    verdict: "challenged".into(),
                    challenge_type: Some("magnitude_inconsistency".into()),
                    reason: "Compared with sourced bat length".into(),
                    evidence_refs: vec!["db:7".into()],
                }],
            )
            .unwrap();
        let stored = store.assistant_claims(10).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].confidence_before, 0.5);
        assert_eq!(stored[0].confidence_after, 0.1);
        assert_eq!(stored[0].evidence_refs, ["db:7"]);
        assert_eq!(
            store
                .relevant_assistant_challenges("cricket bat length", 5)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store.latest_reply_challenges("conversation").unwrap().len(),
            1
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn scout_investigation_and_scholar_job_commit_together_without_duplicates() {
        let path = temp_path();
        let mut store = LearningStore::open(&path).unwrap();
        assert!(
            store
                .enqueue_scout_research(
                    "challenged",
                    "hagia sophia|height",
                    "assistant_claim:42",
                    "What is the height of Hagia Sophia?",
                )
                .unwrap()
        );
        assert!(
            !store
                .enqueue_scout_research(
                    "challenged",
                    "hagia sophia|height",
                    "assistant_claim:43",
                    "What is the height of Hagia Sophia?",
                )
                .unwrap()
        );
        let records = store.scout_investigations().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].trigger_ref, "assistant_claim:42");
        let job = store.claim_scholar_job().unwrap().unwrap();
        assert_eq!(job.id, records[0].scholar_job_id);
        assert_eq!(job.question, records[0].question);
        assert!(store.claim_scholar_job().unwrap().is_none());
        drop(store);
        let reopened = LearningStore::open(&path).unwrap();
        assert!(
            reopened
                .scout_topic_exists("challenged", "hagia sophia|height")
                .unwrap()
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn only_active_explicitly_approved_claims_can_train_the_model() {
        let path = temp_path();
        let mut map = KnowledgeMap::default();
        map.add("Auric fern", "has test colour", "violet", "review source")
            .unwrap();
        let mut store = LearningStore::open(&path).unwrap();
        store.import_knowledge(&map).unwrap();
        assert!(store.approved_model_claim(1).is_err());
        assert!(store.approve_model_claim(2, "reviewed").is_err());
        store
            .approve_model_claim(1, "I checked the source")
            .unwrap();
        assert_eq!(store.approved_model_claim(1).unwrap().object, "violet");
        drop(store);
        let reopened = LearningStore::open(&path).unwrap();
        assert_eq!(
            reopened.approved_model_claim(1).unwrap().subject,
            "Auric fern"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn scholar_evidence_must_support_the_relation_and_object() {
        let claim = ScholarFinding {
            subject: "Hellenic Police".into(),
            relation: "first editor".into(),
            object: "Dimitrios Mallios".into(),
            evidence_text: "Dimitrios Mallios is Chief of the Hellenic Police.".into(),
            confidence: 0.99,
        };
        assert!(!evidence_supports_claim(&claim));
    }

    #[test]
    fn scholar_queue_promotes_only_source_supported_claims() {
        let path = temp_path();
        let mut store = LearningStore::open(&path).unwrap();
        store
            .enqueue_research("conversation", 1, "photosynthesis")
            .unwrap();
        store
            .enqueue_research("conversation", 1, "photosynthesis")
            .unwrap();
        let job = store.claim_scholar_job().unwrap().unwrap();
        assert_eq!(job.question, "photosynthesis");
        assert!(store.claim_scholar_job().unwrap().is_none());
        store
            .finish_scholar_job(
                &job.id,
                "Photosynthesis",
                "https://en.wikipedia.org/wiki/Photosynthesis",
                "Photosynthesis converts light energy into chemical energy.",
                Some(ScholarFinding {
                    subject: "Photosynthesis".into(),
                    relation: "converts".into(),
                    object: "light energy into chemical energy".into(),
                    evidence_text: "Photosynthesis converts light energy into chemical energy."
                        .into(),
                    confidence: 0.98,
                }),
                None,
            )
            .unwrap();
        let claims = store.active_semantic_claims().unwrap();
        assert_eq!(claims.len(), 1);
        assert!(claims[0].sources[0].contains("en.wikipedia.org/wiki/Photosynthesis"));
        assert_eq!(
            store
                .relevant_semantic_claims("What is photosynthesis?", 5)
                .unwrap()[0]
                .source,
            "https://en.wikipedia.org/wiki/Photosynthesis"
        );

        store.enqueue_research("conversation", 2, "Mars").unwrap();
        let second = store.claim_scholar_job().unwrap().unwrap();
        store
            .finish_scholar_job(
                &second.id,
                "Mars",
                "https://en.wikipedia.org/wiki/Mars",
                "Mars is a planet.",
                Some(ScholarFinding {
                    subject: "Mars".into(),
                    relation: "is a".into(),
                    object: "planet".into(),
                    evidence_text: "Mars has oceans.".into(),
                    confidence: 0.99,
                }),
                None,
            )
            .unwrap();
        assert_eq!(store.active_semantic_claims().unwrap().len(), 1);
        assert_eq!(
            store
                .scholar_jobs()
                .unwrap()
                .into_iter()
                .find(|(job, _)| job.id == second.id)
                .unwrap()
                .0
                .status,
            "needs_review"
        );

        store
            .enqueue_research("conversation", 3, "planet Mars")
            .unwrap();
        let third = store.claim_scholar_job().unwrap().unwrap();
        store
            .finish_scholar_job(
                &third.id,
                "Mars",
                "https://en.wikipedia.org/wiki/Mars",
                "Mars is a planet.",
                Some(ScholarFinding {
                    subject: "Mars".into(),
                    relation: "is a".into(),
                    object: "planet".into(),
                    evidence_text: "Mars is a planet.".into(),
                    confidence: 0.8,
                }),
                None,
            )
            .unwrap();
        let finding = store.scholar_findings(&third.id).unwrap().remove(0);
        assert_eq!(finding.status, "needs_review");
        store
            .review_scholar_finding(finding.id, true, "Checked cited excerpt")
            .unwrap();
        assert_eq!(store.active_semantic_claims().unwrap().len(), 2);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn imports_claims_and_is_idempotent() {
        let path = temp_path();
        let _ = std::fs::remove_file(&path);
        let mut map = KnowledgeMap::default();
        let id = map
            .add("Lyra", "name was given by", "the user", "user message 26")
            .unwrap();
        map.revise(
            id,
            "Lyra",
            "name was given by",
            "the user",
            "correction from user message 27",
        )
        .unwrap();

        {
            let mut store = LearningStore::open(&path).unwrap();
            assert_eq!(
                store.import_knowledge(&map).unwrap(),
                ImportSummary {
                    inserted: 2,
                    already_present: 0
                }
            );
            assert_eq!(store.claim_count().unwrap(), 2);
            assert_eq!(
                store.import_knowledge(&map).unwrap(),
                ImportSummary {
                    inserted: 0,
                    already_present: 2
                }
            );
        }

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn staging_same_turn_is_idempotent_and_preserves_raw_text() {
        let path = temp_path();
        let _ = std::fs::remove_file(&path);
        let store = LearningStore::open(&path).unwrap();
        let raw = "  I gave it to you.  ";
        let first = store.stage_observation("conversation-a", 3, raw).unwrap();
        let retry = store.stage_observation("conversation-a", 3, raw).unwrap();
        assert_eq!(first, retry);
        let count: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM observations", [], |row| row.get(0))
            .unwrap();
        let stored: String = store
            .connection
            .query_row(
                "SELECT raw_text FROM observations WHERE id = ?1",
                [&first],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(stored, raw);
        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn pending_observations_are_retrieved_by_relevance_and_conversation() {
        let path = temp_path();
        let _ = std::fs::remove_file(&path);
        let store = LearningStore::open(&path).unwrap();
        store
            .stage_observation("conversation-a", 1, "I live in Bristol.")
            .unwrap();
        store
            .stage_observation("conversation-a", 2, "I like painting.")
            .unwrap();
        store
            .stage_observation("conversation-b", 1, "I live in London.")
            .unwrap();
        let current = store
            .stage_observation("conversation-a", 3, "Where do I live?")
            .unwrap();

        let relevant = store
            .relevant_observations("conversation-a", "Where do I live?", &current, 8)
            .unwrap();
        assert_eq!(relevant.len(), 1);
        assert_eq!(relevant[0].raw_text, "I live in Bristol.");

        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn processing_observations_can_be_reclaimed_after_their_lease_expires() {
        let path = temp_path();
        let _ = std::fs::remove_file(&path);
        let mut store = LearningStore::open(&path).unwrap();
        let id = store
            .stage_observation("conversation-a", 1, "I live in Bristol.")
            .unwrap();
        let first = store.claim_next_observation(600).unwrap().unwrap();
        assert_eq!(first.id, id);
        assert_eq!(first.status, "processing");

        std::thread::sleep(std::time::Duration::from_millis(5));
        drop(store);
        let mut store = LearningStore::open(&path).unwrap();
        let recovered = store.claim_next_observation(0).unwrap().unwrap();
        assert_eq!(recovered.id, id);
        let retry_count: i64 = store
            .connection
            .query_row(
                "SELECT retry_count FROM observations WHERE id = ?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retry_count, 2);

        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn candidate_insert_and_observation_status_commit_together() {
        let path = temp_path();
        let _ = std::fs::remove_file(&path);
        let mut store = LearningStore::open(&path).unwrap();
        let id = store
            .stage_observation("conversation-a", 1, "I live in Bristol.")
            .unwrap();
        store.claim_next_observation(600).unwrap().unwrap();
        let claim = CandidateClaim {
            subject: "the user".into(),
            relation: "lives in".into(),
            object: "Bristol".into(),
            evidence_text: "I live in Bristol".into(),
            context_message_ids: Vec::new(),
            resolution: None,
            confidence: 0.92,
            status: "proposed".into(),
            decision_reason: None,
        };
        store.finish_extraction(&id, &[claim], None).unwrap();
        let observation_status: String = store
            .connection
            .query_row(
                "SELECT status FROM observations WHERE id = ?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        let (candidate_status, evidence): (String, String) = store
            .connection
            .query_row(
                "SELECT status, evidence_text FROM candidate_claims WHERE observation_id = ?1",
                [&id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(observation_status, "needs_review");
        assert_eq!(candidate_status, "needs_review");
        assert_eq!(evidence, "I live in Bristol");

        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn high_confidence_claims_promote_and_equivalent_evidence_attaches() {
        let path = temp_path();
        let _ = std::fs::remove_file(&path);
        let mut store = LearningStore::open(&path).unwrap();
        let first_id = store
            .stage_observation("conversation-a", 1, "I live in Bristol.")
            .unwrap();
        store.claim_next_observation(600).unwrap().unwrap();
        let make_claim = || CandidateClaim {
            subject: " the   user ".into(),
            relation: "lives in".into(),
            object: "Bristol".into(),
            evidence_text: "I live in Bristol.".into(),
            context_message_ids: Vec::new(),
            resolution: None,
            confidence: 0.97,
            status: "proposed".into(),
            decision_reason: None,
        };
        store
            .finish_extraction(&first_id, &[make_claim()], None)
            .unwrap();
        let second_id = store
            .stage_observation("conversation-a", 2, "I live in Bristol.")
            .unwrap();
        store.claim_next_observation(600).unwrap().unwrap();
        store
            .finish_extraction(&second_id, &[make_claim()], None)
            .unwrap();
        let semantic_count: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM semantic_claims", [], |row| row.get(0))
            .unwrap();
        let source_count: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM semantic_claim_sources", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(semantic_count, 1);
        assert_eq!(source_count, 2);
        assert_eq!(
            store.relevant_semantic_claims("Bristol", 5).unwrap().len(),
            1
        );
        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn confirmation_promotes_a_matching_review_candidate_and_keeps_both_sources() {
        let path = temp_path();
        let _ = std::fs::remove_file(&path);
        let mut store = LearningStore::open(&path).unwrap();
        let first_id = store
            .stage_observation("conversation-a", 1, "I live in Shifnal.")
            .unwrap();
        store.claim_next_observation(600).unwrap().unwrap();
        store
            .finish_extraction(
                &first_id,
                &[CandidateClaim {
                    subject: "the user".into(),
                    relation: "lives in".into(),
                    object: "Shifnal".into(),
                    evidence_text: "I live in Shifnal.".into(),
                    context_message_ids: Vec::new(),
                    resolution: None,
                    confidence: 0.0,
                    status: "needs_review".into(),
                    decision_reason: Some("extractor confidence was missing".into()),
                }],
                None,
            )
            .unwrap();

        let confirmation_id = store
            .stage_observation("conversation-a", 2, "Yes, that's right.")
            .unwrap();
        store.claim_next_observation(600).unwrap().unwrap();
        store
            .finish_extraction(
                &confirmation_id,
                &[CandidateClaim {
                    subject: "the user".into(),
                    relation: "lives in".into(),
                    object: "Shifnal".into(),
                    evidence_text: "Yes, that's right.".into(),
                    context_message_ids: vec!["turn:1:user".into(), "turn:1:assistant".into()],
                    resolution: Some("The user confirms the location stated earlier.".into()),
                    confidence: 0.99,
                    status: "proposed".into(),
                    decision_reason: None,
                }],
                None,
            )
            .unwrap();

        let claims = store.active_semantic_claims().unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].claim.object, "Shifnal");
        assert_eq!(claims[0].sources.len(), 2);
        assert!(
            claims[0]
                .sources
                .iter()
                .any(|source| source.contains("I live in Shifnal."))
        );
        assert!(
            claims[0]
                .sources
                .iter()
                .any(|source| source.contains("Yes, that's right."))
        );
        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn self_referential_name_question_finds_a_curated_user_claim() {
        let path = temp_path();
        let _ = std::fs::remove_file(&path);
        let mut store = LearningStore::open(&path).unwrap();
        let id = store
            .stage_observation("conversation-a", 1, "I am Ryan.")
            .unwrap();
        store.claim_next_observation(600).unwrap().unwrap();
        let claim = CandidateClaim {
            subject: "the user".into(),
            relation: "is named".into(),
            object: "Ryan".into(),
            evidence_text: "I am Ryan.".into(),
            context_message_ids: Vec::new(),
            resolution: None,
            confidence: 0.98,
            status: "proposed".into(),
            decision_reason: None,
        };
        store.finish_extraction(&id, &[claim], None).unwrap();
        let matches = store
            .relevant_semantic_claims("Do you know my name?", 5)
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].object, "Ryan");
        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn personal_questions_retrieve_unreviewed_candidate_interpretations() {
        let path = temp_path();
        let _ = std::fs::remove_file(&path);
        let mut store = LearningStore::open(&path).unwrap();
        let id = store
            .stage_observation("conversation-a", 1, "I am Ryan.")
            .unwrap();
        store.claim_next_observation(600).unwrap().unwrap();
        let claim = CandidateClaim {
            subject: "the user".into(),
            relation: "is named".into(),
            object: "Ryan".into(),
            evidence_text: "I am Ryan.".into(),
            context_message_ids: Vec::new(),
            resolution: None,
            confidence: 0.0,
            status: "needs_review".into(),
            decision_reason: Some("missing confidence".into()),
        };
        store.finish_extraction(&id, &[claim], None).unwrap();
        let matches = store
            .relevant_review_candidates("conversation-a", "Do you know my name?", 5)
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].object, "Ryan");
        assert_eq!(matches[0].observation_id, id);
        let current = store
            .stage_observation("conversation-a", 2, "Do you know my name?")
            .unwrap();
        let source = store
            .relevant_observations("conversation-a", "Do you know my name?", &current, 5)
            .unwrap();
        assert_eq!(source.len(), 1);
        assert_eq!(source[0].raw_text, "I am Ryan.");
        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn personal_claim_retrieval_does_not_surface_a_different_user_fact() {
        let path = temp_path();
        let _ = std::fs::remove_file(&path);
        let mut store = LearningStore::open(&path).unwrap();
        let add_review_candidate =
            |store: &mut LearningStore, turn: usize, text: &str, relation: &str, object: &str| {
                let id = store
                    .stage_observation("conversation-a", turn, text)
                    .unwrap();
                store.claim_next_observation(600).unwrap().unwrap();
                store
                    .finish_extraction(
                        &id,
                        &[CandidateClaim {
                            subject: "the user".into(),
                            relation: relation.into(),
                            object: object.into(),
                            evidence_text: text.into(),
                            context_message_ids: Vec::new(),
                            resolution: None,
                            confidence: 0.0,
                            status: "needs_review".into(),
                            decision_reason: Some("missing confidence".into()),
                        }],
                        None,
                    )
                    .unwrap();
            };
        add_review_candidate(&mut store, 1, "I live in Shifnal.", "lives in", "Shifnal");
        add_review_candidate(&mut store, 2, "My name is Ryan.", "is named", "Ryan");

        let matches = store
            .relevant_review_candidates("conversation-a", "What about my name?", 5)
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].object, "Ryan");
        let current = store
            .stage_observation("conversation-a", 3, "What about my name?")
            .unwrap();
        let observations = store
            .relevant_observations("conversation-a", "What about my name?", &current, 5)
            .unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].raw_text, "My name is Ryan.");
        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn contradictory_high_confidence_claim_stays_for_review() {
        let path = temp_path();
        let _ = std::fs::remove_file(&path);
        let mut store = LearningStore::open(&path).unwrap();
        let first_id = store
            .stage_observation("conversation-a", 1, "I live in Bristol.")
            .unwrap();
        store.claim_next_observation(600).unwrap().unwrap();
        let claim = |object: &str, evidence: &str| CandidateClaim {
            subject: "the user".into(),
            relation: "lives in".into(),
            object: object.into(),
            evidence_text: evidence.into(),
            context_message_ids: Vec::new(),
            resolution: None,
            confidence: 0.98,
            status: "proposed".into(),
            decision_reason: None,
        };
        store
            .finish_extraction(&first_id, &[claim("Bristol", "I live in Bristol.")], None)
            .unwrap();
        let second_id = store
            .stage_observation("conversation-a", 2, "I live in London.")
            .unwrap();
        store.claim_next_observation(600).unwrap().unwrap();
        store
            .finish_extraction(&second_id, &[claim("London", "I live in London.")], None)
            .unwrap();
        let (status, reason): (String, Option<String>) = store
            .connection
            .query_row(
                "SELECT status, decision_reason FROM candidate_claims WHERE observation_id = ?1",
                [&second_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "needs_review");
        assert!(
            reason
                .unwrap()
                .contains("conflicts with active semantic claim")
        );
        assert_eq!(store.claim_count().unwrap(), 1);
        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn inbox_details_and_review_acceptance_preserve_conflict_history() {
        let path = temp_path();
        let _ = std::fs::remove_file(&path);
        let mut store = LearningStore::open(&path).unwrap();
        let first_id = store
            .stage_observation("conversation-a", 1, "I live in Bristol.")
            .unwrap();
        store.claim_next_observation(600).unwrap().unwrap();
        let candidate = |object: &str, text: &str, confidence| CandidateClaim {
            subject: "the user".into(),
            relation: "lives in".into(),
            object: object.into(),
            evidence_text: text.into(),
            context_message_ids: Vec::new(),
            resolution: None,
            confidence,
            status: "proposed".into(),
            decision_reason: None,
        };
        store
            .finish_extraction(
                &first_id,
                &[candidate("Bristol", "I live in Bristol.", 0.97)],
                None,
            )
            .unwrap();
        let second_id = store
            .stage_observation("conversation-a", 2, "I live in London.")
            .unwrap();
        store.claim_next_observation(600).unwrap().unwrap();
        store
            .finish_extraction(
                &second_id,
                &[candidate("London", "I live in London.", 0.98)],
                None,
            )
            .unwrap();
        let inbox = store.inbox_summary().unwrap();
        assert_eq!(inbox.needs_review, 1);
        let detail = store.observation_detail(&second_id).unwrap().unwrap();
        assert_eq!(detail.observation.raw_text, "I live in London.");
        let candidate_id = detail.candidates[0].id.clone();
        store
            .resolve_candidate(&candidate_id, true, "The user corrected their location.")
            .unwrap();
        let claims = store.active_semantic_claims().unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].claim.object, "London");
        assert_eq!(
            claims[0].claim.source_observation_id.as_deref(),
            Some(second_id.as_str())
        );
        assert_eq!(claims[0].sources.len(), 1);
        let old_retracted: Option<String> = store
            .connection
            .query_row(
                "SELECT retracted_by FROM semantic_claims WHERE object = 'Bristol'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            old_retracted
                .unwrap()
                .contains("The user corrected their location.")
        );
        assert_eq!(store.inbox_summary().unwrap().needs_review, 0);
        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }
}
