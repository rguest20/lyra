use super::*;
use sha2::{Digest, Sha256};

pub(super) const MAX_RETAINED_SOURCE_CHARS: usize = 32_000;

// Shared candidate gate only. Human origin verification, pair provenance and
// active-claim conflicts remain enforced by their respective callers.
pub(super) const AUTOMATIC_PROMOTION_ELIGIBILITY_CTE: &str = "eligible_findings AS (
    SELECT f.*, a.independence_key
    FROM scholar_findings f
    JOIN scholar_source_assessments a ON a.finding_id = f.id
    WHERE f.status IN ('needs_review', 'promoted')
      AND f.confidence >= ?8
      AND a.source_kind = 'secondary'
      AND a.direct_support = 1
      AND a.independence_verified = 1
      AND (SELECT suitable_for_topic FROM scholar_source_expertise_reviews er
           WHERE er.finding_id = f.id ORDER BY er.id DESC LIMIT 1) = 1
)";

impl ScholarAiReviewTask {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            finding_id: row.get(0)?,
            job_id: row.get(1)?,
        })
    }
}

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

pub(super) fn unix_time_millis() -> Result<u128, std::time::SystemTimeError> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis())
}

pub(super) fn unix_time_ms_i64(overflow_message: &str) -> io::Result<i64> {
    super::unix_time_ms_i64_with_overflow(|_| invalid(overflow_message))
}

pub(super) fn scout_investigations_last_day(connection: &Connection) -> io::Result<i64> {
    connection
        .query_row(
            "SELECT COUNT(*) FROM scout_investigations
             WHERE created_at >= strftime('%Y-%m-%dT%H:%M:%fZ','now','-1 day')",
            [],
            |row| row.get(0),
        )
        .map_err(database_error)
}

pub(super) fn new_scholar_job_id(connection: &Connection) -> io::Result<String> {
    connection
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
        .map_err(database_error)
}

pub(super) fn sql_now_millis(connection: &Connection) -> io::Result<i64> {
    connection
        .query_row(
            "SELECT CAST(strftime('%s','now') AS INTEGER)*1000",
            [],
            |row| row.get(0),
        )
        .map_err(database_error)
}

pub(super) fn mark_scholar_job_processing(connection: &Connection, job_id: &str) -> io::Result<()> {
    connection
        .execute(
            "UPDATE scholar_jobs SET status = 'processing', claimed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), attempts = attempts + 1 WHERE id = ?1",
            [job_id],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(super) fn mark_semantic_claim_challenged(
    connection: &Connection,
    claim_id: i64,
) -> io::Result<()> {
    connection
        .execute(
            "UPDATE semantic_claims SET status = 'challenged' WHERE id = ?1",
            [claim_id],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(super) fn supersede_semantic_claim(
    connection: &Connection,
    claim_id: i64,
    reason: &str,
) -> io::Result<()> {
    connection
        .execute(
            "UPDATE semantic_claims SET retracted_by = ?2, status = 'superseded' WHERE id = ?1",
            params![claim_id, reason],
        )
        .map_err(database_error)?;
    Ok(())
}

fn enqueue_conversation_job(
    connection: &Connection,
    conversation_id: &str,
    turn_number: i64,
    question: &str,
) -> io::Result<()> {
    connection
        .execute(
            "INSERT INTO scholar_jobs (id, conversation_id, turn_number, question, status, created_at)
             VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, 'pending',
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(conversation_id, turn_number) DO NOTHING",
            params![conversation_id, turn_number, question],
        )
        .map_err(database_error)?;
    Ok(())
}

fn saved_semantic_support(connection: &Connection, finding_id: i64) -> io::Result<Option<String>> {
    connection
        .query_row(
            "SELECT assessment_json FROM scholar_semantic_support WHERE finding_id = ?1",
            [finding_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)
}

pub(super) fn set_scholar_task_stop_reason(
    connection: &Connection,
    job_id: &str,
    reason: &str,
) -> io::Result<()> {
    connection
        .execute(
            "UPDATE scholar_task_targets SET stop_reason=?2,
                updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE job_id=?1",
            params![job_id, reason],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(super) fn latest_scholar_finding_proposal_id(
    connection: &Connection,
    finding_id: i64,
) -> io::Result<i64> {
    connection
        .query_row(
            "SELECT id FROM role_proposals WHERE source_type = 'scholar_finding'
             AND source_id = ?1 ORDER BY id DESC LIMIT 1",
            [finding_id.to_string()],
            |row| row.get(0),
        )
        .map_err(database_error)
}

pub(super) fn activate_reviewed_claim(connection: &Connection, claim_id: i64) -> io::Result<()> {
    connection
        .execute(
            "UPDATE semantic_claims SET status = 'active',
                checked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                last_reviewed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                next_review_at = CASE volatility
                    WHEN 'fast_changing' THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+30 days')
                    WHEN 'slow_changing' THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+180 days')
                    ELSE NULL END
             WHERE id = ?1 AND retracted_by IS NULL",
            [claim_id],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(super) struct NewSemanticClaim<'a> {
    pub subject: &'a str,
    pub relation: &'a str,
    pub object: &'a str,
    pub source: &'a str,
    pub supersedes_id: Option<i64>,
    pub valid_from: Option<&'a str>,
    pub valid_until: Option<&'a str>,
    pub proposition: &'a PropositionMetadata,
}

pub(super) fn insert_scholar_semantic_claim(
    tx: &Transaction<'_>,
    claim: NewSemanticClaim<'_>,
) -> io::Result<i64> {
    let NewSemanticClaim {
        subject,
        relation,
        object,
        source,
        supersedes_id,
        valid_from,
        valid_until,
        proposition,
    } = claim;
    tx.execute(
        "INSERT INTO semantic_claims
            (subject, relation, object, source, supersedes_id, source_observation_id, created_at,
             valid_from, valid_until, attribution, polarity, modality,
             geographic_scope, context_scope)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            subject,
            relation,
            object,
            source,
            supersedes_id,
            valid_from,
            valid_until,
            proposition.attribution.as_deref(),
            proposition.polarity.as_str(),
            proposition.modality.as_str(),
            proposition.geographic_scope.as_deref(),
            proposition.context_scope.as_deref(),
        ],
    )
    .map_err(database_error)?;
    Ok(tx.last_insert_rowid())
}

pub(super) fn promote_scholar_finding(
    connection: &Connection,
    finding_id: i64,
    claim_id: i64,
    reason: &str,
) -> io::Result<usize> {
    connection
        .execute(
            "UPDATE scholar_findings SET status = 'promoted', semantic_claim_id = ?2,
                decision_reason = ?3 WHERE id = ?1 AND status = 'needs_review'",
            params![finding_id, claim_id, reason],
        )
        .map_err(database_error)
}

pub(super) fn link_scholar_finding_claim(
    connection: &Connection,
    finding_id: i64,
    claim_id: i64,
    relation: &str,
    rationale: &str,
    assessment_method: &str,
) -> io::Result<()> {
    connection
        .execute(
            "INSERT OR IGNORE INTO scholar_finding_claim_links
                (finding_id,claim_id,relation,rationale,assessment_method)
             VALUES (?1,?2,?3,?4,?5)",
            params![finding_id, claim_id, relation, rationale, assessment_method],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(super) struct NewResearchSource<'a> {
    pub job_id: &'a str,
    pub title: &'a str,
    pub url: &'a str,
    pub content_hash: &'a str,
    pub body: &'a str,
    pub source_scope: &'a str,
    pub input_characters: i64,
    pub retained_characters: i64,
    pub truncated: bool,
    pub provider: &'a str,
    pub query: Option<&'a str>,
    pub published_at: Option<&'a str>,
    pub capture_status: &'a str,
}

pub(super) struct PreparedResearchSource {
    pub body: String,
    pub content_hash: String,
    pub input_characters: i64,
    pub retained_characters: i64,
    pub truncated: bool,
}

pub(super) fn prepare_research_source(
    body: &str,
    maximum_characters: usize,
) -> io::Result<PreparedResearchSource> {
    let input_characters = body.chars().count();
    let retained_body = body.chars().take(maximum_characters).collect::<String>();
    let retained_characters = retained_body.chars().count();
    let truncated = input_characters > retained_characters;
    Ok(PreparedResearchSource {
        content_hash: format!("{:x}", Sha256::digest(body.as_bytes())),
        input_characters: i64::try_from(input_characters).map_err(io::Error::other)?,
        retained_characters: i64::try_from(retained_characters).map_err(io::Error::other)?,
        truncated,
        body: retained_body,
    })
}

pub(super) fn research_source_capture_status(source_scope: &str, truncated: bool) -> &'static str {
    if truncated {
        "truncated"
    } else if source_scope.contains("excerpt") || source_scope.contains("section") {
        "excerpted"
    } else {
        "complete"
    }
}

pub(super) struct SemanticClaimScope {
    pub id: i64,
    pub proposition: ScopedProposition,
}

pub(super) struct ScopedProposition {
    pub object: String,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub geographic_scope: Option<String>,
    pub context_scope: Option<String>,
}

impl ScopedProposition {
    pub(super) fn same_scope(
        &self,
        geographic_scope: &Option<String>,
        context_scope: &Option<String>,
    ) -> bool {
        &self.geographic_scope == geographic_scope && &self.context_scope == context_scope
    }
}

pub(super) struct ScholarPropositionScope {
    pub subject: String,
    pub relation: String,
    pub proposition: ScopedProposition,
}

impl SemanticClaimScope {
    pub fn from_row(row: &rusqlite::Row<'_>, first_column: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(first_column)?,
            proposition: ScopedProposition::from_row(row, first_column + 1)?,
        })
    }
}

impl ScopedProposition {
    pub fn from_row(row: &rusqlite::Row<'_>, first_column: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            object: row.get(first_column)?,
            valid_from: row.get(first_column + 1)?,
            valid_until: row.get(first_column + 2)?,
            geographic_scope: row.get(first_column + 3)?,
            context_scope: row.get(first_column + 4)?,
        })
    }
}

impl ScholarPropositionScope {
    pub fn from_row(row: &rusqlite::Row<'_>, first_column: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            subject: row.get(first_column)?,
            relation: row.get(first_column + 1)?,
            proposition: ScopedProposition::from_row(row, first_column + 2)?,
        })
    }
}

pub(super) fn insert_research_source(
    connection: &Connection,
    source: NewResearchSource<'_>,
) -> io::Result<()> {
    connection
        .execute(
            "INSERT OR IGNORE INTO research_sources
                (job_id,title,url,content_hash,body,created_at,source_scope,
                 input_characters,retained_characters,truncated,provider,query,published_at,
                 capture_status)
             VALUES (?1,?2,?3,?4,?5,strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                 ?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                source.job_id,
                source.title,
                source.url,
                source.content_hash,
                source.body,
                source.source_scope,
                source.input_characters,
                source.retained_characters,
                source.truncated,
                source.provider,
                source.query,
                source.published_at,
                source.capture_status,
            ],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(super) fn evidence_byte_offsets(
    body: &str,
    base_char_offset: usize,
    passage: &str,
    evidence: &str,
) -> Option<(usize, usize)> {
    let prefix_bytes = body
        .chars()
        .take(base_char_offset)
        .map(char::len_utf8)
        .sum::<usize>();
    let local_start = passage.find(evidence)?;
    let start = prefix_bytes.checked_add(local_start)?;
    let end = start.checked_add(evidence.len())?;
    Some((start, end))
}

pub(super) fn evidence_byte_offsets_i64(
    body: &str,
    base_char_offset: usize,
    passage: &str,
    evidence: &str,
) -> io::Result<Option<(i64, i64)>> {
    evidence_byte_offsets(body, base_char_offset, passage, evidence)
        .map(|(start, end)| {
            Ok((
                i64::try_from(start).map_err(io::Error::other)?,
                i64::try_from(end).map_err(io::Error::other)?,
            ))
        })
        .transpose()
}

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
        let daily = scout_investigations_last_day(connection)?;
        if daily >= i64::try_from(daily_limit).unwrap_or(20) {
            return Ok(Some("Scout daily investigation budget exhausted"));
        }
    }
    Ok(None)
}

pub(super) use network::robots_block_key;

impl LearningStore {
    #[cfg(test)]
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
        Ok(sqlite_count(count))
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
        let turn = i64::try_from(turn_number).map_err(|_| invalid("turn number too large"))?;
        enqueue_conversation_job(&self.connection, conversation_id, turn, question)
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
        enqueue_conversation_job(&tx, conversation, turn, &plan.question)?;
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
        let job_id = new_scholar_job_id(&tx)?;
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
            mark_scholar_job_processing(&tx, &job.id)?;
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
        let now = sql_now_millis(&self.connection)?;
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
        set_scholar_task_stop_reason(&self.connection, job_id, stop_reason)
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
                    source_span: ScholarEvidenceSpan::from_row(row, 1, 8)?,
                    subject: row.get(5)?,
                    relation: row.get(6)?,
                    object: row.get(7)?,
                    confidence: row.get(9)?,
                    status: row.get(10)?,
                    decision_reason: row.get(11)?,
                    valid_from: row.get(12)?,
                    valid_until: row.get(13)?,
                    source_quality: row.get(14)?,
                    freshness: row.get(15)?,
                    source_updated_at: row.get(16)?,
                    proposition: PropositionMetadata::from_storage_row(row, 17)?,
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
            let support = saved_semantic_support(&self.connection, finding_id)?;
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

    pub(crate) fn scholar_ai_review_queue(&self) -> io::Result<Vec<ScholarAiReviewTask>> {
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
            .query_map([], ScholarAiReviewTask::from_row)
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
        let store = crate::test_support::in_memory_learning_store();
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
        let store = crate::test_support::in_memory_learning_store();
        store
            .enqueue_research("lyra-frontend", 7, "Which flavours does Walkers sell?")
            .unwrap();
        let previous = store.scout_questions_for_subject("Walkers").unwrap();
        assert_eq!(previous, ["Which flavours does Walkers sell?"]);
    }

    #[test]
    fn duplicate_lead_explains_the_no_result_cooldown() {
        let store = crate::test_support::in_memory_learning_store();
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
        let store = crate::test_support::in_memory_learning_store();
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
        let directory = crate::test_support::unique_temp_path("lyra-deferred-scout", "");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("learning.db");
        let mut store = LearningStore::open(&path).unwrap();
        let probe_id = store
            .begin_model_probe("Inca roads", "fixture")
            .unwrap()
            .unwrap();
        let raw =
            "TOPIC: Inca roads\nQUESTION: What materials are used in constructing Inca roads?";
        assert!(
            store
                .defer_model_probe(
                    probe_id,
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
                    probe_id,
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
        let deferred_probe = reopened.deferred_model_probe().unwrap().unwrap();
        let id = deferred_probe.id;
        let topic = deferred_probe.topic;
        let question = deferred_probe.question;
        let raw = deferred_probe.raw_output;
        assert_eq!(id, probe_id);
        assert_eq!(deferred_probe.seed, "Inca roads");
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

struct LatestScoutTopic {
    status: String,
    age_seconds: Option<f64>,
    attempts: i64,
}

fn scout_topic_block_reason(
    connection: &Connection,
    trigger_kind: &str,
    topic_key: &str,
) -> io::Result<Option<&'static str>> {
    let latest = connection
        .query_row(
            "SELECT j.status, (julianday('now') - julianday(s.created_at)) * 86400,
                (SELECT COUNT(*) FROM scout_investigations attempts
                 WHERE attempts.trigger_kind = s.trigger_kind AND attempts.topic_key = s.topic_key)
             FROM scout_investigations s JOIN scholar_jobs j ON j.id = s.scholar_job_id
             WHERE s.trigger_kind = ?1 AND s.topic_key = ?2
             ORDER BY s.id DESC LIMIT 1",
            params![trigger_kind, topic_key],
            |row| {
                Ok(LatestScoutTopic {
                    status: row.get(0)?,
                    age_seconds: row.get(1)?,
                    attempts: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(database_error)?;
    Ok(match latest {
        None => None,
        Some(latest) if latest.attempts >= 3 => {
            Some("Scout topic has reached its three-investigation attempt limit")
        }
        Some(latest) if matches!(latest.status.as_str(), "pending" | "processing") => {
            Some("Same Scout investigation is already pending or processing")
        }
        Some(latest) if latest.status == "promoted" => {
            Some("Scout topic already has promoted research")
        }
        Some(latest) => {
            let (seconds, reason) = if latest.status == "no_result" {
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
            latest
                .age_seconds
                .is_none_or(|value| value < seconds)
                .then_some(reason)
        }
    })
}
