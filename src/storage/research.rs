use super::*;

pub(super) mod baskets;
mod corroboration;
mod discovery;
mod feedback;
mod intake;
mod model_probes;
pub(crate) mod network;
mod promotion;
mod reviews;
use promotion::self_promote_verified_findings;

#[derive(Debug)]
pub(crate) enum ScoutAdmission {
    Queued(String),
    Deferred(&'static str),
    Blocked(&'static str),
}

fn scout_daily_limit() -> u64 {
    std::env::var("COMMUNITY_MAX_RESEARCH_PER_DAY")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value <= 200)
        .unwrap_or(20)
}

fn scout_queue_block_reason(
    connection: &Connection,
    min_interval: u64,
    daily_limit: u64,
) -> io::Result<Option<&'static str>> {
    if min_interval > 0 {
        let recent: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM scout_investigations
            WHERE created_at > strftime('%Y-%m-%dT%H:%M:%fZ','now',?1))",
                [format!("-{min_interval} seconds")],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if recent {
            return Ok(Some("Scout minimum investigation interval has not elapsed"));
        }
    }
    if daily_limit > 0 {
        let daily: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM scout_investigations
            WHERE created_at >= strftime('%Y-%m-%dT%H:%M:%fZ','now','-1 day')",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if daily >= i64::try_from(daily_limit).unwrap_or(20) {
            return Ok(Some("Scout daily investigation budget exhausted"));
        }
    }
    Ok(None)
}

pub(super) use network::robots_block_key;

impl LearningStore {
    pub fn scholar_task_target(&self, job_id: &str) -> io::Result<Option<ScholarTaskTarget>> {
        let raw: Option<(String, String, String, String, String, Option<String>)> = self
            .connection
            .query_row(
                "SELECT original_question,entity_constraints_json,scope_constraints_json,
                    evidence_gap,acceptable_evidence_json,stop_reason
                 FROM scholar_task_targets WHERE job_id=?1",
                [job_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?;
        let Some((original_question, entities, scopes, evidence_gap, acceptable, stop_reason)) =
            raw
        else {
            return Ok(None);
        };
        Ok(Some(ScholarTaskTarget {
            job_id: job_id.to_owned(),
            original_question,
            entity_constraints: serde_json::from_str(&entities).map_err(invalid_json)?,
            scope_constraints: serde_json::from_str(&scopes).map_err(invalid_json)?,
            evidence_gap,
            acceptable_evidence: serde_json::from_str(&acceptable).map_err(invalid_json)?,
            sources_tried: self.scholar_search_attempts(job_id)?,
            stop_reason,
        }))
    }

    pub fn set_scholar_task_constraints(
        &self,
        job_id: &str,
        entities: &[String],
        scopes: &[String],
    ) -> io::Result<()> {
        if entities.len() > 4
            || scopes.len() > 8
            || entities
                .iter()
                .chain(scopes)
                .any(|value| value.trim().is_empty() || value.chars().count() > 200)
        {
            return Err(invalid("invalid Scholar task constraints"));
        }
        self.connection.execute(
            "UPDATE scholar_task_targets SET entity_constraints_json=?2,scope_constraints_json=?3,
                updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE job_id=?1",
            params![job_id,serde_json::to_string(entities).map_err(invalid_json)?,serde_json::to_string(scopes).map_err(invalid_json)?],
        ).map_err(database_error)?;
        Ok(())
    }

    pub fn scout_questions_for_subject(&self, subject: &str) -> io::Result<Vec<String>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT question FROM (
                 SELECT s.question AS question, s.created_at AS recency
                 FROM scout_investigations s
                 JOIN semantic_claims c ON s.trigger_ref = 'db:' || c.id
                 WHERE lower(trim(c.subject)) = lower(trim(?1))
                 UNION ALL
                 SELECT q.question AS question, q.created_at AS recency
                 FROM research_questions q
                 JOIN research_sources source ON source.id = q.source_id
                 WHERE instr(lower(trim(source.title)), lower(trim(?1))) > 0
                    OR instr(lower(trim(?1)), lower(trim(source.title))) > 0
                 UNION ALL
                 SELECT j.question AS question, j.created_at AS recency
                 FROM scholar_jobs j
                 WHERE NOT EXISTS(SELECT 1 FROM scout_investigations s WHERE s.scholar_job_id=j.id)
                   AND NOT EXISTS(SELECT 1 FROM scholar_research_feedback f WHERE f.scholar_job_id=j.id)
                   AND (instr(lower(j.question),lower(?1)) > 0 OR instr(lower(?1),lower(j.question)) > 0)
             )
             GROUP BY lower(trim(question))
             ORDER BY MAX(recency) DESC LIMIT 10",
            )
            .map_err(database_error)?;
        statement
            .query_map([subject], |row| row.get(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn scout_expansion_count_for_subject(&self, subject: &str) -> io::Result<usize> {
        let count: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM scout_investigations s
             JOIN semantic_claims c ON s.trigger_ref = 'db:' || c.id
             WHERE s.trigger_kind = 'expansion' AND lower(trim(c.subject)) = lower(trim(?1))",
                [subject],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        Ok(usize::try_from(count).unwrap_or(usize::MAX))
    }

    pub fn scout_known_claims_for_subject(&self, subject: &str) -> io::Result<Vec<String>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT relation || ': ' || object FROM semantic_claims
             WHERE retracted_by IS NULL AND status IN ('active', 'uncertain', 'challenged')
               AND lower(trim(subject)) = lower(trim(?1))
             ORDER BY id DESC LIMIT 8",
            )
            .map_err(database_error)?;
        statement
            .query_map([subject], |row| row.get(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn scholar_job_is_expansion(&self, job_id: &str) -> io::Result<bool> {
        self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM scout_investigations WHERE scholar_job_id = ?1 AND trigger_kind = 'expansion')",
            [job_id],
            |row| row.get(0),
        ).map_err(database_error)
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

    pub fn enqueue_research_plan(
        &self,
        conversation: &str,
        turn: usize,
        plan: &crate::research::ResearchPlan,
    ) -> io::Result<()> {
        let turn = i64::try_from(turn).map_err(|_| invalid("turn number too large"))?;
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(database_error)?;
        tx.execute("INSERT INTO scholar_jobs (id, conversation_id, turn_number, question, status, created_at)
            VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, 'pending', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            ON CONFLICT(conversation_id, turn_number) DO NOTHING", params![conversation,turn,plan.question]).map_err(database_error)?;
        tx.execute(
            "INSERT INTO scholar_lookup_plans (job_id, queries_json)
            SELECT id, ?3 FROM scholar_jobs WHERE conversation_id = ?1 AND turn_number = ?2
            ON CONFLICT(job_id) DO NOTHING",
            params![
                conversation,
                turn,
                serde_json::to_string(&plan.queries).map_err(invalid_json)?
            ],
        )
        .map_err(database_error)?;
        tx.commit().map_err(database_error)
    }

    pub fn scholar_lookup_queries(&self, job: &str) -> io::Result<Option<Vec<String>>> {
        let raw: Option<String> = self
            .connection
            .query_row(
                "SELECT queries_json FROM scholar_lookup_plans WHERE job_id = ?1",
                [job],
                |r| r.get(0),
            )
            .optional()
            .map_err(database_error)?;
        raw.map(|raw| serde_json::from_str(&raw).map_err(invalid_json))
            .transpose()
    }

    /// Atomically record Scout's reason and enqueue the corresponding Scholar job.
    pub fn enqueue_scout_research_with_reason(
        &mut self,
        trigger_kind: &str,
        topic_key: &str,
        trigger_ref: &str,
        question: &str,
        selection_reason: &str,
    ) -> io::Result<bool> {
        match self.try_enqueue_scout_research(
            trigger_kind,
            topic_key,
            trigger_ref,
            question,
            selection_reason,
        )? {
            ScoutAdmission::Queued(_) => Ok(true),
            ScoutAdmission::Deferred(reason) | ScoutAdmission::Blocked(reason) => {
                crate::diagnostics::event(
                    "scout.queue_deferred",
                    serde_json::json!({"trigger_kind":trigger_kind,"reason":reason,"policy":self.scout_queue_status()?}),
                );
                Ok(false)
            }
        }
    }

    pub fn try_enqueue_scout_research(
        &mut self,
        trigger_kind: &str,
        topic_key: &str,
        trigger_ref: &str,
        question: &str,
        selection_reason: &str,
    ) -> io::Result<ScoutAdmission> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        if let Some(reason) = scout_topic_block_reason(&tx, trigger_kind, topic_key)? {
            return Ok(ScoutAdmission::Blocked(reason));
        }
        if scout_question_already_attempted(&tx, question)? {
            return Ok(ScoutAdmission::Blocked(
                "Same question is pending or was researched within the last day",
            ));
        }
        if let Some(reason) =
            scout_queue_block_reason(&tx, scout_min_interval_seconds(), scout_daily_limit())?
        {
            return Ok(ScoutAdmission::Deferred(reason));
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
             (trigger_kind, topic_key, trigger_ref, question, scholar_job_id, selection_reason, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![trigger_kind, topic_key, trigger_ref, question, job_id, selection_reason],
        )
        .map_err(database_error)?;
        let evidence_refs = serde_json::to_string(&[trigger_ref]).map_err(invalid_json)?;
        let payload = serde_json::to_string(&serde_json::json!({
            "trigger_kind": trigger_kind,
            "topic_key": topic_key,
            "question": question,
            "scholar_job_id": job_id,
            "selection_reason": selection_reason,
        }))
        .map_err(invalid_json)?;
        let proposal_id = record_role_proposal(
            &tx,
            RoleProposal {
                role: "scout",
                source_type: "scout_investigation",
                source_id: &job_id,
                suggested_action: "queue_research",
                target_claim_id: trigger_ref
                    .strip_prefix("claim:")
                    .and_then(|id| id.parse().ok()),
                evidence_refs: &evidence_refs,
                payload: &payload,
            },
        )?;
        record_policy_decision(
            &tx,
            proposal_id,
            "queued",
            "bounded Scout investigation accepted by queue policy",
            trigger_ref
                .strip_prefix("claim:")
                .and_then(|id| id.parse().ok()),
            Some("research_queued"),
        )?;
        tx.commit().map_err(database_error)?;
        Ok(ScoutAdmission::Queued(job_id))
    }

    pub fn scout_investigations(&self) -> io::Result<Vec<ScoutInvestigation>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT trigger_kind, topic_key, trigger_ref, question, scholar_job_id, selection_reason
             FROM scout_investigations ORDER BY id DESC LIMIT 30",
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
                    selection_reason: row.get(5)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn scout_topic_exists(&self, trigger_kind: &str, topic_key: &str) -> io::Result<bool> {
        scout_topic_blocked(&self.connection, trigger_kind, topic_key)
    }

    pub fn claim_scholar_job(&mut self) -> io::Result<Option<ScholarJob>> {
        self.claim_scholar_job_with_priority(false, None)
    }

    /// Direct conversation questions are exposed to foreground workers first.
    pub fn claim_user_scholar_job(&mut self) -> io::Result<Option<ScholarJob>> {
        self.claim_scholar_job_with_priority(true, None)
    }

    pub fn claim_conversation_research(
        &mut self,
        conversation: &str,
        turn: usize,
    ) -> io::Result<Option<ScholarJob>> {
        self.claim_scholar_job_with_priority(true, Some((conversation, turn)))
    }

    pub fn has_pending_user_research(&self) -> io::Result<bool> {
        self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM scholar_jobs j
                 WHERE ((j.status = 'pending' AND (j.next_attempt_at_ms <= CAST(strftime('%s','now') AS INTEGER)*1000
                     OR j.last_error = 'Scholar reached the daily external page-fetch limit')) OR (j.status = 'processing'
                AND j.claimed_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-600 seconds')))
             AND NOT EXISTS(SELECT 1 FROM scout_investigations s WHERE s.scholar_job_id = j.id)
             AND NOT EXISTS(SELECT 1 FROM scholar_research_feedback f WHERE f.scholar_job_id = j.id))",
            [], |row| row.get(0),
        ).map_err(database_error)
    }

    fn claim_scholar_job_with_priority(
        &mut self,
        user_only: bool,
        target: Option<(&str, usize)>,
    ) -> io::Result<Option<ScholarJob>> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let job = tx.query_row(
            "WITH queued AS (
               SELECT j.*, j.rowid AS queue_order,
                 (NOT EXISTS(SELECT 1 FROM scout_investigations s WHERE s.scholar_job_id = j.id)
                  AND NOT EXISTS(SELECT 1 FROM scholar_research_feedback f WHERE f.scholar_job_id = j.id)) AS user_requested,
                 EXISTS(SELECT 1 FROM research_questions rq WHERE rq.scholar_job_id = j.id) AS source_derived
                             FROM scholar_jobs j WHERE (status = 'pending' AND (next_attempt_at_ms <= CAST(strftime('%s','now') AS INTEGER)*1000
                                 OR last_error = 'Scholar reached the daily external page-fetch limit'))
               OR (status = 'processing' AND claimed_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-600 seconds'))
             )
             SELECT id, question, status FROM queued WHERE (?1 = 0 OR user_requested)
               AND (?2 IS NULL OR (conversation_id = ?2 AND turn_number = ?3))
             ORDER BY user_requested DESC, source_derived DESC,
               CASE WHEN user_requested THEN created_at END DESC,
               CASE WHEN user_requested THEN queue_order END DESC,
               created_at, queue_order LIMIT 1",
            params![user_only, target.map(|t| t.0), target.map(|t| i64::try_from(t.1)).transpose().map_err(|_| invalid("turn number too large"))?],
            |row| Ok(ScholarJob { id: row.get(0)?, question: row.get(1)?, status: row.get(2)? }),
        ).optional().map_err(database_error)?;
        if let Some(job) = &job {
            tx.execute("UPDATE scholar_jobs SET status = 'processing', claimed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), attempts = attempts + 1 WHERE id = ?1", [&job.id]).map_err(database_error)?;
        }
        tx.commit().map_err(database_error)?;
        Ok(job)
    }

    pub fn defer_scholar_job(
        &self,
        job_id: &str,
        reason: &str,
        retry_at_ms: i64,
    ) -> io::Result<()> {
        self.connection.execute(
            "UPDATE scholar_jobs SET status='pending',last_error=?2,claimed_at=NULL,next_attempt_at_ms=?3
             WHERE id=?1 AND status='processing'",
            params![job_id,reason.chars().take(500).collect::<String>(),retry_at_ms],
        ).map_err(database_error)?;
        Ok(())
    }

    pub fn next_research_retry_ms(&self) -> io::Result<i64> {
        let now: i64 = self
            .connection
            .query_row(
                "SELECT CAST(strftime('%s','now') AS INTEGER)*1000",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let backoff: i64 = self
            .connection
            .query_row(
                "SELECT COALESCE(MAX(retry_until_ms),0) FROM scholar_host_backoff",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        Ok(self.wikipedia_budget_retry_ms(now)?.max(backoff))
    }

    pub fn fail_scholar_job(&self, job_id: &str, reason: &str) -> io::Result<()> {
        self.connection
            .execute(
                "UPDATE scholar_jobs SET status = 'no_result', last_error = ?2, claimed_at = NULL
                 WHERE id = ?1 AND status = 'processing'",
                params![job_id, reason.chars().take(500).collect::<String>()],
            )
            .map_err(database_error)?;
        let stop_reason = if reason.contains("extraction") {
            "extraction_failure"
        } else if reason.contains("entity constraints") {
            "target_definition_failure"
        } else {
            "processing_failure"
        };
        self.connection.execute(
            "UPDATE scholar_task_targets SET stop_reason=?2,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE job_id=?1",
            params![job_id, stop_reason],
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
                "SELECT id, source_archive_id, evidence_byte_start, evidence_byte_end,
                    source_url, subject, relation, object, evidence_text,
                    confidence, status, decision_reason, valid_from, valid_until,
                    source_quality, freshness, source_updated_at, attribution, polarity, modality,
                    geographic_scope, context_scope
             FROM scholar_findings WHERE job_id = ?1 ORDER BY id",
            )
            .map_err(database_error)?;
        statement
            .query_map([job_id], |row| {
                Ok(StoredScholarFinding {
                    id: row.get(0)?,
                    source_archive_id: row.get(1)?,
                    evidence_byte_start: row.get(2)?,
                    evidence_byte_end: row.get(3)?,
                    source_url: row.get(4)?,
                    subject: row.get(5)?,
                    relation: row.get(6)?,
                    object: row.get(7)?,
                    evidence_text: row.get(8)?,
                    confidence: row.get(9)?,
                    status: row.get(10)?,
                    decision_reason: row.get(11)?,
                    valid_from: row.get(12)?,
                    valid_until: row.get(13)?,
                    source_quality: row.get(14)?,
                    freshness: row.get(15)?,
                    source_updated_at: row.get(16)?,
                    attribution: row.get(17)?,
                    polarity: row.get(18)?,
                    modality: row.get(19)?,
                    geographic_scope: row.get(20)?,
                    context_scope: row.get(21)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn scholar_source_assessment(
        &self,
        finding_id: i64,
    ) -> io::Result<Option<ScholarSourceAssessment>> {
        self.connection
            .query_row(
                "SELECT finding_id, topic_class, source_kind, topical_expertise,
                    direct_support, independence_key, independence_verified,
                    currency, published_at, gate_status, reason, research_relevance
             FROM scholar_source_assessments WHERE finding_id = ?1",
                [finding_id],
                |row| {
                    Ok(ScholarSourceAssessment {
                        finding_id: row.get(0)?,
                        topic_class: row.get(1)?,
                        source_kind: row.get(2)?,
                        topical_expertise: row.get(3)?,
                        direct_support: row.get::<_, Option<i64>>(4)?.map(|value| value != 0),
                        independence_key: row.get(5)?,
                        independence_verified: row.get::<_, i64>(6)? != 0,
                        currency: row.get(7)?,
                        published_at: row.get(8)?,
                        gate_status: row.get(9)?,
                        reason: row.get(10)?,
                        research_relevance: row.get(11)?,
                    })
                },
            )
            .optional()
            .map_err(database_error)
    }

    pub fn scholar_review_context(&self, finding_id: i64) -> io::Result<Option<serde_json::Value>> {
        let mut context = self
            .connection
            .query_row(
                "SELECT f.id, f.job_id, j.question, f.source_title, f.source_url,
                    f.source_excerpt, f.subject, f.relation, f.object, f.evidence_text,
                    f.confidence, f.valid_from, f.valid_until, f.source_quality,
                    a.direct_support, a.gate_status, a.reason, a.research_relevance,
                    f.freshness, f.source_updated_at, a.topic_class,
                    f.attribution, f.polarity, f.modality, f.geographic_scope, f.context_scope
                 FROM scholar_findings f
                 JOIN scholar_jobs j ON j.id = f.job_id
                 JOIN scholar_source_assessments a ON a.finding_id = f.id
                 WHERE f.id = ?1 AND f.status = 'needs_review'
                   AND (NOT EXISTS (
                          SELECT 1 FROM scholar_source_expertise_reviews er
                          WHERE er.finding_id = f.id AND er.review_method = 'automatic'
                        ) OR a.gate_status = 'conflict')",
                [finding_id],
                |row| {
                    Ok(serde_json::json!({
                        "finding_id": row.get::<_, i64>(0)?,
                        "job_id": row.get::<_, String>(1)?,
                        "question": row.get::<_, String>(2)?,
                        "source_title": row.get::<_, String>(3)?,
                        "source_url": row.get::<_, String>(4)?,
                        "source_excerpt": row.get::<_, String>(5)?,
                        "subject": row.get::<_, String>(6)?,
                        "relation": row.get::<_, String>(7)?,
                        "object": row.get::<_, String>(8)?,
                        "evidence_text": row.get::<_, String>(9)?,
                        "confidence": row.get::<_, f64>(10)?,
                        "valid_from": row.get::<_, Option<String>>(11)?,
                        "valid_until": row.get::<_, Option<String>>(12)?,
                        "source_quality": row.get::<_, String>(13)?,
                        "direct_support": row.get::<_, Option<i64>>(14)? == Some(1),
                        "gate_status": row.get::<_, String>(15)?,
                        "gate_reason": row.get::<_, String>(16)?,
                        "research_relevance": row.get::<_, String>(17)?,
                        "freshness": row.get::<_, String>(18)?,
                        "source_updated_at": row.get::<_, Option<String>>(19)?,
                        "topic_class": row.get::<_, String>(20)?,
                        "attribution": row.get::<_, Option<String>>(21)?,
                        "polarity": row.get::<_, String>(22)?,
                        "modality": row.get::<_, String>(23)?,
                        "geographic_scope": row.get::<_, Option<String>>(24)?,
                        "context_scope": row.get::<_, Option<String>>(25)?,
                    }))
                },
            )
            .optional()
            .map_err(database_error)?;
        if let Some(context) = context.as_mut() {
            let support: Option<String> = self
                .connection
                .query_row(
                    "SELECT assessment_json FROM scholar_semantic_support WHERE finding_id = ?1",
                    [finding_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(database_error)?;
            context["support_method"] = if support.is_some() {
                "ai_semantic_entailment"
            } else {
                "lexical"
            }
            .into();
            context["semantic_support"] = support
                .map(|raw| serde_json::from_str::<serde_json::Value>(&raw).map_err(invalid_json))
                .transpose()?
                .unwrap_or(serde_json::Value::Null);
            let subject = context["subject"].as_str().unwrap_or_default().to_owned();
            let relation = context["relation"].as_str().unwrap_or_default().to_owned();
            let object = context["object"].as_str().unwrap_or_default().to_owned();
            let mut statement = self
                .connection
                .prepare(
                    "SELECT id, object, source, status, valid_from, valid_until
                     FROM semantic_claims WHERE retracted_by IS NULL
                       AND lower(trim(subject)) = lower(trim(?1))
                       AND lower(trim(relation)) = lower(trim(?2))
                       AND lower(trim(object)) <> lower(trim(?3))",
                )
                .map_err(database_error)?;
            let conflicts = statement
                .query_map(params![&subject, &relation, &object], |row| {
                    Ok(serde_json::json!({
                        "claim_id": row.get::<_, i64>(0)?,
                        "object": row.get::<_, String>(1)?,
                        "source": row.get::<_, String>(2)?,
                        "status": row.get::<_, String>(3)?,
                        "valid_from": row.get::<_, Option<String>>(4)?,
                        "valid_until": row.get::<_, Option<String>>(5)?,
                    }))
                })
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?;
            context["conflicting_active_claims"] = serde_json::json!(conflicts);
            let mut statement = self
                .connection
                .prepare(
                    "SELECT f.id, f.source_url, f.source_excerpt, f.evidence_text,
                        f.confidence, a.independence_key, a.gate_status
                     FROM scholar_findings f
                     JOIN scholar_source_assessments a ON a.finding_id = f.id
                     WHERE f.id <> ?1
                       AND lower(trim(f.subject)) = lower(trim(?2))
                       AND lower(trim(f.relation)) = lower(trim(?3))
                       AND lower(trim(f.object)) = lower(trim(?4))
                       AND f.status IN ('needs_review', 'promoted')
                       AND a.direct_support = 1
                     ORDER BY f.id DESC LIMIT 4",
                )
                .map_err(database_error)?;
            let other_support = statement
                .query_map(params![finding_id, &subject, &relation, &object], |row| {
                    Ok(serde_json::json!({
                        "finding_id": row.get::<_, i64>(0)?,
                        "source_url": row.get::<_, String>(1)?,
                        "source_excerpt": row.get::<_, String>(2)?,
                        "evidence_text": row.get::<_, String>(3)?,
                        "confidence": row.get::<_, f64>(4)?,
                        "independence_key": row.get::<_, Option<String>>(5)?,
                        "gate_status": row.get::<_, String>(6)?,
                    }))
                })
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?;
            context["other_supporting_findings"] = serde_json::json!(other_support);
        }
        Ok(context)
    }

    pub fn scholar_corroboration_exhausted(&self, finding_id: i64) -> io::Result<bool> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM scholar_corroboration_queue
                 WHERE finding_id = ?1 AND status = 'exhausted')",
                [finding_id],
                |row| row.get(0),
            )
            .map_err(database_error)
    }

    pub fn scholar_ai_review_queue(&self) -> io::Result<Vec<(i64, String)>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT f.id, f.job_id FROM scholar_findings f
                 WHERE f.status = 'needs_review'
                   AND NOT EXISTS (
                       SELECT 1 FROM scholar_ai_review_retries r
                       WHERE r.finding_id = f.id
                         AND r.retry_after > strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                   )
                   AND EXISTS (SELECT 1 FROM scholar_source_assessments a
                               WHERE a.finding_id = f.id)
                   AND (NOT EXISTS (
                          SELECT 1 FROM scholar_source_expertise_reviews er
                          WHERE er.finding_id = f.id AND er.review_method = 'automatic'
                        ) OR EXISTS (
                          SELECT 1 FROM scholar_source_assessments a
                          WHERE a.finding_id = f.id AND a.gate_status = 'conflict'
                        ) OR EXISTS (
                          SELECT 1 FROM scholar_corroboration_queue q
                          WHERE q.finding_id = f.id AND q.status = 'exhausted'
                        ))
                 ORDER BY f.id LIMIT 1000",
            )
            .map_err(database_error)?;
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn defer_scholar_ai_review(&self, finding_id: i64, reason: &str) -> io::Result<()> {
        self.connection.execute(
            "INSERT INTO scholar_ai_review_retries (finding_id, attempts, retry_after, last_error)
             VALUES (?1, 1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+5 minutes'), ?2)
             ON CONFLICT(finding_id) DO UPDATE SET
                attempts = attempts + 1,
                retry_after = excluded.retry_after,
                last_error = excluded.last_error",
            params![finding_id, reason.chars().take(500).collect::<String>()],
        ).map_err(database_error)?;
        Ok(())
    }
}

#[cfg(test)]
mod scout_admission_tests {
    use super::*;

    #[test]
    fn scout_question_history_includes_questions_retained_from_matching_sources() {
        let store = LearningStore::open(Path::new(":memory:")).unwrap();
        store
            .enqueue_research_plan(
                "question-history",
                1,
                &crate::research::ResearchPlan {
                    question: "What is known about the Orion Nebula?".into(),
                    queries: vec!["Orion Nebula".into()],
                },
            )
            .unwrap();
        let job: String = store
            .connection
            .query_row("SELECT id FROM scholar_jobs LIMIT 1", [], |row| row.get(0))
            .unwrap();
        store
            .retain_research_source(
                &job,
                "Orion Nebula",
                "https://example.org/orion",
                "The Orion Nebula contains young stars.",
            )
            .unwrap();
        let source: i64 = store
            .connection
            .query_row("SELECT id FROM research_sources LIMIT 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        store.connection.execute(
            "INSERT INTO research_questions(source_id,question,question_key,evidence_text,depth,status,created_at)
             VALUES (?1,?2,?3,?4,0,'researched',strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
            params![source,"How many young stars does the Orion Nebula contain?","how many young stars does the orion nebula contain?","The Orion Nebula contains young stars."],
        ).unwrap();

        let previous = store
            .scout_questions_for_subject("The Orion Nebula")
            .unwrap();
        assert!(previous.contains(&"How many young stars does the Orion Nebula contain?".into()));
        assert!(previous.contains(&"What is known about the Orion Nebula?".into()));
    }

    #[test]
    fn scout_question_history_includes_direct_conversation_questions() {
        let store = LearningStore::open(Path::new(":memory:")).unwrap();
        store
            .enqueue_research("lyra-frontend", 7, "Which flavours does Walkers sell?")
            .unwrap();
        let previous = store.scout_questions_for_subject("Walkers").unwrap();
        assert_eq!(previous, ["Which flavours does Walkers sell?"]);
    }

    #[test]
    fn duplicate_lead_explains_the_no_result_cooldown() {
        let store = LearningStore::open(Path::new(":memory:")).unwrap();
        store
            .enqueue_research_plan(
                "topic-cooldown",
                1,
                &crate::research::ResearchPlan {
                    question: "What materials are used in constructing Inca roads?".into(),
                    queries: vec!["Inca roads materials".into()],
                },
            )
            .unwrap();
        store
            .connection
            .execute("UPDATE scholar_jobs SET status='no_result'", [])
            .unwrap();
        store.connection.execute("INSERT INTO scout_investigations(trigger_kind,topic_key,trigger_ref,question,scholar_job_id,selection_reason,created_at)
            SELECT 'model_probe','inca roads|materials','fixture',question,id,'fixture',strftime('%Y-%m-%dT%H:%M:%fZ','now') FROM scholar_jobs",[]).unwrap();
        assert_eq!(
            scout_topic_block_reason(&store.connection, "model_probe", "inca roads|materials")
                .unwrap(),
            Some("Previous investigation found no result; this topic has a one-day retry cooldown")
        );
        store.connection.execute("UPDATE scout_investigations SET created_at=strftime('%Y-%m-%dT%H:%M:%fZ','now','-2 days')",[]).unwrap();
        assert_eq!(
            scout_topic_block_reason(&store.connection, "model_probe", "inca roads|materials")
                .unwrap(),
            None
        );
    }

    #[test]
    fn scheduling_blocks_explain_cooldown_and_daily_limits() {
        let store = LearningStore::open(Path::new(":memory:")).unwrap();
        store
            .enqueue_research_plan(
                "admission",
                1,
                &crate::research::ResearchPlan {
                    question: "What are Inca roads made of?".into(),
                    queries: vec!["Inca roads materials".into()],
                },
            )
            .unwrap();
        store.connection.execute("INSERT INTO scout_investigations(trigger_kind,topic_key,trigger_ref,question,scholar_job_id,selection_reason,created_at)
            SELECT 'model_probe','fixture','fixture',question,id,'fixture',strftime('%Y-%m-%dT%H:%M:%fZ','now') FROM scholar_jobs",[]).unwrap();
        assert_eq!(
            scout_queue_block_reason(&store.connection, 300, 20).unwrap(),
            Some("Scout minimum investigation interval has not elapsed")
        );
        assert_eq!(
            scout_queue_block_reason(&store.connection, 0, 1).unwrap(),
            Some("Scout daily investigation budget exhausted")
        );
        assert_eq!(
            scout_queue_block_reason(&store.connection, 0, 0).unwrap(),
            None
        );
    }

    #[test]
    fn deferred_model_lead_survives_restart_and_reuses_its_probe() {
        let directory = std::env::temp_dir().join(format!(
            "lyra-deferred-scout-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("learning.db");
        let mut store = LearningStore::open(&path).unwrap();
        let probe = store
            .begin_model_probe("Inca roads", "fixture")
            .unwrap()
            .unwrap();
        let raw =
            "TOPIC: Inca roads\nQUESTION: What materials are used in constructing Inca roads?";
        assert!(
            store
                .defer_model_probe(
                    probe,
                    raw,
                    "Inca roads",
                    "What materials are used in constructing Inca roads?",
                    "Scout daily investigation budget exhausted"
                )
                .unwrap()
        );
        assert!(
            !store
                .defer_model_probe(
                    probe,
                    raw,
                    "Inca roads",
                    "What materials are used in constructing Inca roads?",
                    "Scout daily investigation budget exhausted"
                )
                .unwrap()
        );
        assert!(store.recent_model_topics().unwrap().is_empty());
        drop(store);
        let mut reopened = LearningStore::open(&path).unwrap();
        let (id, _, topic, question, raw) = reopened.deferred_model_probe().unwrap().unwrap();
        assert_eq!(id, probe);
        let ScoutAdmission::Queued(job) = reopened
            .try_enqueue_scout_research(
                "model_probe",
                "inca roads|materials",
                &format!("model_probe:{id}"),
                &question,
                "Deferred lead resumed",
            )
            .unwrap()
        else {
            panic!("fresh queue must admit the stored lead")
        };
        reopened
            .complete_model_probe(
                id,
                "queued",
                &raw,
                Some(&topic),
                Some(&question),
                Some(&job),
            )
            .unwrap();
        assert!(reopened.deferred_model_probe().unwrap().is_none());
        assert_eq!(reopened.model_probes().unwrap().len(), 1);
        assert_eq!(reopened.recent_model_topics().unwrap(), vec!["Inca roads"]);
        drop(reopened);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

fn scout_topic_blocked(
    connection: &Connection,
    trigger_kind: &str,
    topic_key: &str,
) -> io::Result<bool> {
    Ok(scout_topic_block_reason(connection, trigger_kind, topic_key)?.is_some())
}

fn scout_question_already_attempted(connection: &Connection, question: &str) -> io::Result<bool> {
    // Key changes must not evade recent-question cooldowns. Use exact word
    // sequences rather than topic overlap: nearby but different questions
    // remain distinct. This applies only to autonomous admission; direct user
    // requests and bounded feedback repairs have their own authorized paths.
    let signature = |text: &str| {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
    };
    let requested = signature(question);
    let mut statement = connection
        .prepare(
            "SELECT question FROM scholar_jobs WHERE status IN ('pending','processing')
         OR created_at >= strftime('%Y-%m-%dT%H:%M:%fZ','now','-1 day')",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(database_error)?;
    for row in rows {
        if signature(&row.map_err(database_error)?) == requested {
            return Ok(true);
        }
    }
    Ok(false)
}

fn scout_topic_block_reason(
    connection: &Connection,
    trigger_kind: &str,
    topic_key: &str,
) -> io::Result<Option<&'static str>> {
    let latest: Option<(String, Option<f64>, i64)> = connection
        .query_row(
            "SELECT j.status, (julianday('now') - julianday(s.created_at)) * 86400,
                (SELECT COUNT(*) FROM scout_investigations attempts
                 WHERE attempts.trigger_kind = s.trigger_kind AND attempts.topic_key = s.topic_key)
             FROM scout_investigations s JOIN scholar_jobs j ON j.id = s.scholar_job_id
             WHERE s.trigger_kind = ?1 AND s.topic_key = ?2
             ORDER BY s.id DESC LIMIT 1",
            params![trigger_kind, topic_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(database_error)?;
    Ok(match latest {
        None => None,
        Some((_, _, attempts)) if attempts >= 3 => {
            Some("Scout topic has reached its three-investigation attempt limit")
        }
        Some((status, _, _)) if matches!(status.as_str(), "pending" | "processing") => {
            Some("Same Scout investigation is already pending or processing")
        }
        Some((status, _, _)) if status == "promoted" => {
            Some("Scout topic already has promoted research")
        }
        Some((status, age, _)) => {
            let (seconds, reason) = if status == "no_result" {
                (
                    86_400.0,
                    "Previous investigation found no result; this topic has a one-day retry cooldown",
                )
            } else {
                (
                    604_800.0,
                    "Previous investigation awaits review or corroboration; this topic has a seven-day retry cooldown",
                )
            };
            age.is_none_or(|value| value < seconds).then_some(reason)
        }
    })
}
