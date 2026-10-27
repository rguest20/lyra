use super::*;

const RESEARCH_HARVEST_CHUNK_CHARS: usize = 2_000;
const RESEARCH_HARVEST_STRIDE_CHARS: usize = 1_000;

#[derive(Debug)]
pub struct ResearchSource {
    pub id: i64,
    pub title: String,
    pub url: String,
    pub passage: String,
    pub cursor: i64,
    pub next_cursor: i64,
}

struct RetainedSourceImport {
    job_id: String,
    title: String,
    url: String,
    body: String,
}

struct ClaimableResearchSource {
    id: i64,
    title: String,
    url: String,
    body: String,
    cursor: i64,
}

struct ResearchSourceHarvestState {
    body: String,
    job_id: String,
    cursor: i64,
}

struct ResearchSourceCandidateContext {
    job_id: String,
    provider: String,
    query: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RetainedClaim {
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub evidence_text: String,
    #[serde(default)]
    pub qualifications: String,
    #[serde(default, alias = "confidence")]
    pub extraction_confidence: f64,
}

struct RetainedResearchQuestion {
    id: i64,
    source_id: i64,
    question: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RetainedQuestion {
    pub question: String,
    pub evidence_text: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ResearchBaskets {
    pub claims: Vec<RetainedClaim>,
    #[serde(default)]
    pub questions: Vec<RetainedQuestion>,
}

#[derive(Debug, Serialize)]
pub struct ClaimPair {
    pub left_id: i64,
    pub right_id: i64,
    pub left: serde_json::Value,
    pub right: serde_json::Value,
}

struct RetainedClaimEvidence {
    subject: String,
    relation: String,
    object: String,
    quotation: String,
    qualifications: String,
    confidence: f64,
    source_url: String,
    source_id: i64,
    byte_start: Option<i64>,
    byte_end: Option<i64>,
    content_hash: String,
}

fn now_ms() -> io::Result<i64> {
    crate::storage::unix_time_ms_i64_with_overflow(io::Error::other)
}

fn semantic_subject_terms(subject: &str) -> Vec<String> {
    const STOP_WORDS: &[&str] = &[
        "the", "a", "an", "and", "or", "of", "is", "are", "was", "were", "in", "on", "at", "to",
        "from", "for", "by", "with", "into", "that", "this", "which", "who", "what", "how", "when",
        "where", "does", "has", "have", "be", "been", "being", "as", "its", "their", "it", "they",
        "can", "may", "will", "would",
    ];
    let mut terms = subject
        .chars()
        .flat_map(char::to_lowercase)
        .map(|ch| if ch.is_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .filter(|term| term.chars().count() >= 3 && !STOP_WORDS.contains(term))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    terms.sort();
    terms.dedup();
    if terms.is_empty() {
        terms.push("__empty__".into());
    }
    terms
}

impl LearningStore {
    /// Persistently rotates background Scout work so available queues get a
    /// turn even when another source of work remains continuously eligible.
    pub fn claim_next_scout_stage(&mut self) -> io::Result<usize> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let stage: i64 = tx
            .query_row(
                "SELECT next_stage FROM scout_work_state WHERE id=1",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        tx.execute(
            "UPDATE scout_work_state SET next_stage=?1 WHERE id=1",
            [(stage + 1) % 5],
        )
        .map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        usize::try_from(stage).map_err(io::Error::other)
    }

    pub fn next_research_assertion_lead(
        &mut self,
    ) -> io::Result<Option<crate::storage::RetainedClaimLead>> {
        self.connection.execute(
            "WITH claim_depth AS (
                SELECT c.id AS claim_id,
                    COALESCE((SELECT parent.derivation_depth+1
                        FROM scout_investigations i
                        JOIN research_assertion_checks parent
                          ON i.trigger_ref='research_claim:' || parent.claim_id
                        WHERE i.trigger_kind='assertion' AND i.scholar_job_id=s.job_id
                        ORDER BY i.id DESC LIMIT 1),
                      (SELECT max(q.depth) FROM research_questions q WHERE q.scholar_job_id=s.job_id),0) AS depth
                FROM research_claims c JOIN research_sources s ON s.id=c.source_id
                WHERE NOT EXISTS(SELECT 1 FROM research_assertion_checks a WHERE a.claim_id=c.id)
             )
             INSERT OR IGNORE INTO research_assertion_checks
                (claim_id,question,selection_reason,status,derivation_depth,created_at,updated_at)
             SELECT claim_id,NULL,'Skipped: maximum assertion derivation depth is three','skipped',depth,
                strftime('%Y-%m-%dT%H:%M:%fZ','now'),strftime('%Y-%m-%dT%H:%M:%fZ','now')
             FROM claim_depth WHERE depth>=3",
            [],
        ).map_err(database_error)?;
        self.connection
            .query_row(
                "SELECT c.id,c.subject,c.relation,c.object,c.evidence_text,c.qualifications,
                    s.title,s.url,s.content_hash,
                    COALESCE((SELECT parent.derivation_depth+1
                        FROM scout_investigations i
                        JOIN research_assertion_checks parent
                          ON i.trigger_ref='research_claim:' || parent.claim_id
                        WHERE i.trigger_kind='assertion' AND i.scholar_job_id=s.job_id
                        ORDER BY i.id DESC LIMIT 1),
                      (SELECT max(q.depth) FROM research_questions q WHERE q.scholar_job_id=s.job_id),0)
                 FROM research_claims c JOIN research_sources s ON s.id=c.source_id
                 WHERE NOT EXISTS(SELECT 1 FROM research_assertion_checks a WHERE a.claim_id=c.id)
                   AND COALESCE((SELECT parent.derivation_depth+1
                        FROM scout_investigations i
                        JOIN research_assertion_checks parent
                          ON i.trigger_ref='research_claim:' || parent.claim_id
                        WHERE i.trigger_kind='assertion' AND i.scholar_job_id=s.job_id
                        ORDER BY i.id DESC LIMIT 1),
                      (SELECT max(q.depth) FROM research_questions q WHERE q.scholar_job_id=s.job_id),0) < 3
                 ORDER BY c.id LIMIT 1",
                [],
                |row| {
                    Ok(crate::storage::RetainedClaimLead {
                        claim_id: row.get(0)?,
                        subject: row.get(1)?,
                        relation: row.get(2)?,
                        object: row.get(3)?,
                        evidence_text: row.get(4)?,
                        qualifications: row.get(5)?,
                        source_title: row.get(6)?,
                        source_url: row.get(7)?,
                        content_hash: row.get(8)?,
                        derivation_depth: row.get(9)?,
                    })
                },
            )
            .optional()
            .map_err(database_error)
    }

    pub fn has_pending_research_assertion_lead(&self) -> io::Result<bool> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM research_assertion_checks WHERE status='pending')",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)
    }

    pub fn recent_research_assertion_checks(
        &self,
        limit: i64,
    ) -> io::Result<Vec<serde_json::Value>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT c.subject,c.relation,c.object,c.qualifications,c.evidence_text,
                a.question,a.status,a.selection_reason,a.derivation_depth,s.content_hash
             FROM research_assertion_checks a
             JOIN research_claims c ON c.id=a.claim_id
             JOIN research_sources s ON s.id=c.source_id
             ORDER BY a.updated_at DESC,a.claim_id DESC LIMIT ?1",
            )
            .map_err(database_error)?;
        statement
            .query_map([limit.clamp(1, 30)], |row| {
                Ok(serde_json::json!({
                    "subject":row.get::<_,String>(0)?,
                    "relation":row.get::<_,String>(1)?,
                    "object":row.get::<_,String>(2)?,
                    "qualifications":row.get::<_,String>(3)?,
                    "exact_source_quote":row.get::<_,String>(4)?,
                    "previous_question":row.get::<_,Option<String>>(5)?,
                    "status":row.get::<_,String>(6)?,
                    "selection_reason":row.get::<_,String>(7)?,
                    "derivation_depth":row.get::<_,i64>(8)?,
                    "source_content_hash":row.get::<_,String>(9)?,
                }))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn research_assertion_selection(&self, claim_id: i64) -> io::Result<String> {
        self.connection
            .query_row(
                "SELECT CASE WHEN question IS NULL THEN 'skipped' ELSE status END
                 FROM research_assertion_checks WHERE claim_id=?1",
                [claim_id],
                |row| row.get(0),
            )
            .map_err(database_error)
    }

    pub fn record_research_assertion_selection(
        &self,
        lead: &crate::storage::RetainedClaimLead,
        question: Option<&str>,
        reason: &str,
    ) -> io::Result<()> {
        if !crate::storage::valid_printable_field(reason, 1_000)
            || question.is_some_and(|question| !crate::storage::valid_bounded_field(question, 300))
        {
            return Err(invalid("invalid retained assertion selection"));
        }
        self.connection.execute(
                "INSERT OR IGNORE INTO research_assertion_checks
                (claim_id,question,selection_reason,status,derivation_depth,created_at,updated_at)
             VALUES (?1,?2,?3,?4,?5,strftime('%Y-%m-%dT%H:%M:%fZ','now'),strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
            params![lead.claim_id,question,reason,if question.is_some() { "pending" } else { "skipped" },lead.derivation_depth],
        ).map_err(database_error)?;
        Ok(())
    }

    pub fn queue_research_assertion_lead(&mut self) -> io::Result<Option<(i64, String)>> {
        self.connection.execute(
            "UPDATE research_assertion_checks
             SET status=CASE
                 WHEN EXISTS(SELECT 1 FROM scholar_findings f WHERE f.job_id=scholar_job_id AND f.status='promoted')
                   OR (SELECT status FROM scholar_jobs WHERE id=scholar_job_id)='promoted' THEN 'verified'
                 WHEN EXISTS(SELECT 1 FROM scholar_findings f JOIN scholar_source_assessments a ON a.finding_id=f.id
                     WHERE f.job_id=scholar_job_id AND a.gate_status='awaiting_corroboration') THEN 'waiting_for_evidence'
                 WHEN EXISTS(SELECT 1 FROM scholar_findings f WHERE f.job_id=scholar_job_id AND f.status='needs_review') THEN 'proposed'
                 WHEN EXISTS(SELECT 1 FROM scholar_findings f WHERE f.job_id=scholar_job_id AND f.status='rejected') THEN 'reviewed'
                 WHEN (SELECT status FROM scholar_jobs WHERE id=scholar_job_id)='needs_review' THEN 'proposed'
                 WHEN (SELECT status FROM scholar_jobs WHERE id=scholar_job_id)='no_result' THEN 'unanswered'
                 ELSE status END,
                 updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE status IN ('queued','proposed','waiting_for_evidence') AND scholar_job_id IN
                (SELECT id FROM scholar_jobs WHERE status IN ('promoted','needs_review','no_result'))",
            [],
        ).map_err(database_error)?;
        let lead: Option<(i64, String, String)> = self
            .connection
            .query_row(
                "SELECT claim_id,question,selection_reason FROM research_assertion_checks
             WHERE status='pending' ORDER BY claim_id LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(database_error)?;
        let Some((claim_id, question, reason)) = lead else {
            return Ok(None);
        };
        let key = format!("research-claim:{claim_id}");
        let admission = self.try_enqueue_scout_research(
            "assertion",
            &key,
            &format!("research_claim:{claim_id}"),
            &question,
            &reason,
        )?;
        match admission {
            ScoutAdmission::Queued(job_id) => {
                self.connection.execute("UPDATE research_assertion_checks SET status='queued',scholar_job_id=?2,last_admission_reason=NULL,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE claim_id=?1",params![claim_id,job_id]).map_err(database_error)?;
                crate::diagnostics::event(
                    "research.assertion_queued",
                    serde_json::json!({"claim_id":claim_id,"scholar_job_id":job_id}),
                );
                Ok(Some((claim_id, job_id)))
            }
            ScoutAdmission::Deferred(reason) => {
                self.connection.execute("UPDATE research_assertion_checks SET last_admission_reason=?2,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE claim_id=?1",params![claim_id,reason]).map_err(database_error)?;
                crate::diagnostics::event(
                    "research.assertion_deferred",
                    serde_json::json!({"claim_id":claim_id,"reason":reason}),
                );
                Ok(None)
            }
            ScoutAdmission::Blocked(reason) => {
                self.connection.execute("UPDATE research_assertion_checks SET status='skipped',last_admission_reason=?2,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE claim_id=?1",params![claim_id,reason]).map_err(database_error)?;
                crate::diagnostics::event(
                    "research.assertion_skipped",
                    serde_json::json!({"claim_id":claim_id,"reason":reason}),
                );
                Ok(None)
            }
        }
    }

    pub fn bootstrap_research_sources(&self) -> io::Result<i64> {
        let populated: bool = self
            .connection
            .query_row("SELECT EXISTS(SELECT 1 FROM research_sources)", [], |row| {
                row.get(0)
            })
            .map_err(database_error)?;
        if populated {
            return Ok(0);
        }
        self.import_retained_research_sources()
    }

    pub fn import_retained_research_sources(&self) -> io::Result<i64> {
        let before: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM research_sources", [], |row| {
                row.get(0)
            })
            .map_err(database_error)?;
        let mut statement = self.connection.prepare(
            "SELECT job_id,source_title,source_url,source_excerpt FROM scholar_findings
             UNION SELECT job_id,title,COALESCE(fetched_url,url),fetched_excerpt FROM scholar_search_candidates
                WHERE fetched_excerpt IS NOT NULL AND fetched_excerpt<>'' LIMIT 500"
        ).map_err(database_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(RetainedSourceImport {
                    job_id: row.get(0)?,
                    title: row.get(1)?,
                    url: row.get(2)?,
                    body: row.get(3)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        drop(statement);
        for source in rows {
            self.retain_research_source(&source.job_id, &source.title, &source.url, &source.body)?;
        }
        let after: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM research_sources", [], |row| {
                row.get(0)
            })
            .map_err(database_error)?;
        Ok(after - before)
    }

    pub fn retain_fetched_candidate_source(
        &self,
        candidate: i64,
        title: &str,
        url: &str,
        body: &str,
    ) -> io::Result<()> {
        let source_context = self
            .connection
            .query_row(
                "SELECT job_id,provider,query FROM scholar_search_candidates WHERE id=?1",
                [candidate],
                |row| {
                    Ok(ResearchSourceCandidateContext {
                        job_id: row.get(0)?,
                        provider: row.get(1)?,
                        query: row.get(2)?,
                    })
                },
            )
            .map_err(database_error)?;
        self.retain_research_source_with_metadata(super::super::ResearchSourceInput {
            job_id: &source_context.job_id,
            title,
            url,
            body,
            provider: &source_context.provider,
            source_scope: "fetched_page_excerpt",
            query: Some(&source_context.query),
            published_at: None,
        })
    }

    pub fn retain_research_source(
        &self,
        job: &str,
        title: &str,
        url: &str,
        body: &str,
    ) -> io::Result<()> {
        self.retain_research_source_with_metadata(super::super::ResearchSourceInput {
            job_id: job,
            title,
            url,
            body,
            provider: "unknown",
            source_scope: "provided_excerpt",
            query: None,
            published_at: None,
        })
    }

    pub fn retain_research_source_with_metadata(
        &self,
        source: super::super::ResearchSourceInput<'_>,
    ) -> io::Result<()> {
        let parsed =
            url::Url::parse(source.url).map_err(|_| invalid("invalid research source URL"))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(invalid("invalid research source URL"));
        }
        if source.body.trim().is_empty() {
            return Ok(());
        }
        let retained = super::prepare_research_source(source.body, MAX_RETAINED_SOURCE_CHARS)?;
        let capture_status =
            super::research_source_capture_status(source.source_scope, retained.truncated);
        super::insert_research_source(
            &self.connection,
            super::NewResearchSource {
                job_id: source.job_id,
                title: source.title,
                url: source.url,
                content_hash: &retained.content_hash,
                body: &retained.body,
                source_scope: source.source_scope,
                input_characters: retained.input_characters,
                retained_characters: retained.retained_characters,
                truncated: retained.truncated,
                provider: source.provider,
                query: source.query,
                published_at: source.published_at,
                capture_status,
            },
        )
    }

    pub fn claim_research_source(&mut self) -> io::Result<Option<ResearchSource>> {
        let now = now_ms()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let source = tx
            .query_row(
                "SELECT id, title, url, body, harvest_cursor FROM research_sources
                 WHERE harvest_cursor < length(body)
                   AND claimed_until_ms <= ?1 AND next_attempt_at_ms <= ?1
                 ORDER BY id LIMIT 1",
                [now],
                |row| {
                    Ok(ClaimableResearchSource {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        url: row.get(2)?,
                        body: row.get(3)?,
                        cursor: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(database_error)?;
        let Some(ClaimableResearchSource {
            id,
            title,
            url,
            body,
            cursor,
        }) = source
        else {
            return Ok(None);
        };
        let offset =
            usize::try_from(cursor).map_err(|_| invalid("invalid research source cursor"))?;
        let mut passage = body
            .chars()
            .skip(offset)
            .take(RESEARCH_HARVEST_CHUNK_CHARS)
            .collect::<String>();
        if body.chars().count() > offset + passage.chars().count()
            && let Some(end) = passage
                .rfind('\n')
                .filter(|end| *end > RESEARCH_HARVEST_CHUNK_CHARS / 2)
        {
            passage.truncate(end + 1);
        }
        let remaining = body.chars().count().saturating_sub(offset);
        let advance = remaining.min(RESEARCH_HARVEST_STRIDE_CHARS);
        let next_cursor = cursor + i64::try_from(advance).map_err(io::Error::other)?;
        tx.execute(
            "UPDATE research_sources SET claimed_until_ms=?2 WHERE id=?1",
            params![id, now + 600_000],
        )
        .map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        Ok(Some(ResearchSource {
            id,
            title,
            url,
            passage,
            cursor,
            next_cursor,
        }))
    }

    pub fn finish_research_baskets(
        &mut self,
        source: &ResearchSource,
        baskets: &ResearchBaskets,
    ) -> io::Result<(usize, usize)> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let harvest_state = tx
            .query_row(
                "SELECT body,job_id,harvest_cursor FROM research_sources WHERE id=?1",
                [source.id],
                |row| {
                    Ok(ResearchSourceHarvestState {
                        body: row.get(0)?,
                        job_id: row.get(1)?,
                        cursor: row.get(2)?,
                    })
                },
            )
            .map_err(database_error)?;
        let offset = usize::try_from(harvest_state.cursor)
            .map_err(|_| invalid("invalid research source cursor"))?;
        let expected_advance = harvest_state
            .body
            .chars()
            .count()
            .saturating_sub(offset)
            .min(RESEARCH_HARVEST_STRIDE_CHARS);
        if harvest_state.cursor != source.cursor
            || source.next_cursor
                != source.cursor + i64::try_from(expected_advance).map_err(io::Error::other)?
            || harvest_state
                .body
                .chars()
                .skip(offset)
                .take(source.passage.chars().count())
                .collect::<String>()
                != source.passage
        {
            return Err(invalid("research source cursor or passage changed"));
        }
        let mut claims = 0;
        let mut skipped_claims = 0;
        for claim in &baskets.claims {
            if !crate::storage::valid_proposition_evidence(
                &claim.subject,
                &claim.relation,
                &claim.object,
                &claim.evidence_text,
                &source.passage,
            ) || claim.qualifications.chars().count() > 500
                || !crate::storage::valid_confidence(claim.extraction_confidence)
            {
                skipped_claims += 1;
                continue;
            }
            let Some((evidence_byte_start, evidence_byte_end)) = super::evidence_byte_offsets_i64(
                &harvest_state.body,
                offset,
                &source.passage,
                &claim.evidence_text,
            )
            .ok()
            .flatten() else {
                skipped_claims += 1;
                continue;
            };
            claims += tx.execute("INSERT OR IGNORE INTO research_claims(source_id,subject,relation,object,subject_key,relation_key,evidence_text,qualifications,extraction_confidence,created_at,evidence_byte_start,evidence_byte_end)
                VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,strftime('%Y-%m-%dT%H:%M:%fZ','now'),?10,?11)",
                params![source.id,claim.subject.trim(),claim.relation.trim(),claim.object.trim(),canonical(&claim.subject),canonical(&claim.relation),claim.evidence_text,claim.qualifications,claim.extraction_confidence,evidence_byte_start,evidence_byte_end]).map_err(database_error)?;
        }
        let depth: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(depth),0)+1 FROM research_questions WHERE scholar_job_id=?1",
                [&harvest_state.job_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let mut questions = 0;
        let mut skipped_questions = 0;
        for question in &baskets.questions {
            if !crate::storage::valid_bounded_field(&question.question, 300)
                || !crate::storage::valid_source_quote(&question.evidence_text, &source.passage)
            {
                skipped_questions += 1;
                continue;
            }
            if crate::research::private_fact_request(&question.question)
                || question.question.to_ascii_lowercase().contains("the user")
                || depth > 3
            {
                continue;
            }
            let key = canonical(&question.question);
            let Some((evidence_byte_start, evidence_byte_end)) = super::evidence_byte_offsets_i64(
                &harvest_state.body,
                offset,
                &source.passage,
                &question.evidence_text,
            )
            .ok()
            .flatten() else {
                skipped_questions += 1;
                continue;
            };
            questions += tx.execute("INSERT OR IGNORE INTO research_questions(source_id,question,question_key,evidence_text,depth,created_at)
                VALUES (?1,?2,?3,?4,?5,strftime('%Y-%m-%dT%H:%M:%fZ','now'))",params![source.id,question.question.trim(),key,question.evidence_text,depth]).map_err(database_error)?;
            tx.execute("UPDATE research_questions SET evidence_byte_start=COALESCE(evidence_byte_start,?2), evidence_byte_end=COALESCE(evidence_byte_end,?3) WHERE question_key=?1",params![key,evidence_byte_start,evidence_byte_end]).map_err(database_error)?;
            tx.execute("INSERT OR IGNORE INTO research_question_sources(question_id,source_id,evidence_text,evidence_byte_start,evidence_byte_end)
                SELECT id,?2,?3,?4,?5 FROM research_questions WHERE question_key=?1",params![key,source.id,question.evidence_text,evidence_byte_start,evidence_byte_end]).map_err(database_error)?;
        }
        tx.execute("UPDATE research_sources SET harvest_cursor=?2,claimed_until_ms=0,last_error=NULL,next_attempt_at_ms=0 WHERE id=?1",params![source.id,source.next_cursor]).map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        if skipped_claims > 0 || skipped_questions > 0 {
            crate::diagnostics::event(
                "research.basket_items_skipped",
                serde_json::json!({"source_id":source.id,"claims":skipped_claims,"questions":skipped_questions}),
            );
        }
        Ok((claims, questions))
    }

    pub fn defer_research_source(&self, id: i64, reason: &str) -> io::Result<()> {
        self.schedule_source_retry(id, reason, now_ms()? + 300_000)
    }

    /// Preserve an exhausted source for explicit retry, without another timed cycle.
    pub fn park_research_source(&self, id: i64, reason: &str) -> io::Result<()> {
        self.schedule_source_retry(id, reason, i64::MAX)
    }

    fn schedule_source_retry(&self, id: i64, reason: &str, retry_at_ms: i64) -> io::Result<()> {
        self.connection.execute("UPDATE research_sources SET claimed_until_ms=0,next_attempt_at_ms=?2,last_error=?3 WHERE id=?1",
            params![id,retry_at_ms,reason.chars().take(500).collect::<String>()]).map_err(database_error)?;
        Ok(())
    }

    pub fn queue_retained_question(&mut self) -> io::Result<bool> {
        self.connection.execute("UPDATE research_questions SET status=CASE
                WHEN (SELECT status FROM scholar_jobs WHERE id=scholar_job_id)='no_result' THEN 'unanswered' ELSE 'researched' END
            WHERE status='queued' AND scholar_job_id IN (SELECT id FROM scholar_jobs WHERE status IN ('promoted','needs_review','no_result'))",[]).map_err(database_error)?;
        if self.has_pending_user_research()? {
            return Ok(false);
        }
        let queued: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM research_questions WHERE status='queued'",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if queued >= 20 {
            return Ok(false);
        }
        let question = self
            .connection
            .query_row(
                "SELECT id, source_id, question FROM research_questions
                 WHERE status = 'pending' ORDER BY id LIMIT 1",
                [],
                |row| {
                    Ok(RetainedResearchQuestion {
                        id: row.get(0)?,
                        source_id: row.get(1)?,
                        question: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(database_error)?;
        let Some(RetainedResearchQuestion {
            id,
            source_id,
            question,
        }) = question
        else {
            return Ok(false);
        };
        let key = format!("source-question:{id}");
        if !self.enqueue_scout_research_with_reason(
            "expansion",
            &key,
            &format!("source:{source_id}"),
            &question,
            "Retained source passage raises a grounded follow-up question",
        )? {
            return Ok(false);
        }
        self.connection.execute("UPDATE research_questions SET status='queued',scholar_job_id=(SELECT scholar_job_id FROM scout_investigations WHERE trigger_kind='expansion' AND topic_key=?2 ORDER BY id DESC LIMIT 1) WHERE id=?1",params![id,key]).map_err(database_error)?;
        crate::diagnostics::event(
            "research.question_queued",
            serde_json::json!({"question_id":id,"source_id":source_id}),
        );
        Ok(true)
    }

    pub fn has_pending_retained_question(&self) -> io::Result<bool> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM research_questions WHERE status='pending')",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)
    }

    pub fn next_research_claim_pair(&self) -> io::Result<Option<ClaimPair>> {
        let unindexed = {
            let mut statement = self
                .connection
                .prepare(
                    "SELECT c.id,c.subject FROM research_claims c
                     WHERE NOT EXISTS (SELECT 1 FROM research_claim_terms t WHERE t.claim_id=c.id)",
                )
                .map_err(database_error)?;
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?
        };
        for (claim_id, subject) in unindexed {
            for term in semantic_subject_terms(&subject) {
                self.connection
                    .execute(
                        "INSERT OR IGNORE INTO research_claim_terms(term,claim_id) VALUES (?1,?2)",
                        params![term, claim_id],
                    )
                    .map_err(database_error)?;
            }
        }
        self.connection.query_row(
            "SELECT a.id,b.id,a.subject,a.relation,a.object,a.evidence_text,a.qualifications,sa.url,
                b.subject,b.relation,b.object,b.evidence_text,b.qualifications,sb.url,
                EXISTS (SELECT 1 FROM entity_equivalences e WHERE
                    (instr(a.subject_key,e.left_name)>0 AND instr(b.subject_key,e.right_name)>0)
                    OR (instr(a.subject_key,e.right_name)>0 AND instr(b.subject_key,e.left_name)>0))
             FROM research_claims a JOIN research_claims b ON a.id<b.id AND (
                    a.subject_key=b.subject_key
                    OR (length(a.subject_key)>=8 AND length(b.subject_key)>=8 AND
                        (instr(a.subject_key,b.subject_key)>0 OR instr(b.subject_key,a.subject_key)>0))
                    OR EXISTS (
                        SELECT 1 FROM research_claim_terms ta
                        JOIN research_claim_terms tb ON tb.term=ta.term
                        WHERE ta.claim_id=a.id AND tb.claim_id=b.id AND ta.term<>'__empty__'
                    )
                    OR EXISTS (
                        SELECT 1 FROM entity_equivalences e
                        WHERE (e.left_name=a.subject_key AND e.right_name=b.subject_key)
                           OR (e.left_name=b.subject_key AND e.right_name=a.subject_key)
                    )
                    OR EXISTS (
                        SELECT 1 FROM entity_equivalences e
                        WHERE length(e.left_name)>=4 AND length(e.right_name)>=4 AND (
                            (instr(a.subject_key,e.left_name)>0 AND instr(b.subject_key,e.right_name)>0)
                            OR (instr(a.subject_key,e.right_name)>0 AND instr(b.subject_key,e.left_name)>0)
                        )
                    )
                    OR EXISTS (
                        SELECT 1 FROM research_claims identity_claim
                        WHERE identity_claim.relation_key IN (
                            'is also known as','also known as','is known as','alias of',
                            'is the same entity as','same entity as','identical to','formerly known as'
                        ) AND (
                            (identity_claim.subject_key=a.subject_key AND lower(trim(identity_claim.object))=b.subject_key)
                            OR (identity_claim.subject_key=b.subject_key AND lower(trim(identity_claim.object))=a.subject_key)
                        )
                    )
                )
             JOIN research_sources sa ON sa.id=a.source_id JOIN research_sources sb ON sb.id=b.source_id
             LEFT JOIN research_evidence_links l ON l.left_claim_id=a.id AND l.right_claim_id=b.id
             WHERE (l.left_claim_id IS NULL OR (l.relation='insufficient' AND l.next_attempt_at_ms>0 AND l.next_attempt_at_ms<=?1))
                 AND (l.left_claim_id IS NOT NULL OR
                    ((SELECT COUNT(*) FROM research_evidence_links WHERE left_claim_id=a.id OR right_claim_id=a.id)<8
                    AND (SELECT COUNT(*) FROM research_evidence_links WHERE left_claim_id=b.id OR right_claim_id=b.id)<8))
             ORDER BY a.id,b.id LIMIT 1",[now_ms()?],|row|{
                let same_subject_alias: bool = row.get(14)?;
                let claim = |offset| -> rusqlite::Result<serde_json::Value> {Ok(serde_json::json!({
                    "subject":row.get::<_,String>(offset)?,"relation":row.get::<_,String>(offset+1)?,"object":row.get::<_,String>(offset+2)?,
                    "evidence_text":row.get::<_,String>(offset+3)?,"qualifications":row.get::<_,String>(offset+4)?,"source_url":row.get::<_,String>(offset+5)?,
                    "validated_subject_alias":same_subject_alias,
                }))};
                Ok(ClaimPair {left_id:row.get(0)?,right_id:row.get(1)?,left:claim(2)?,right:claim(8)?})
            },
        ).optional().map_err(database_error)
    }

    pub fn record_research_evidence_link(
        &self,
        pair: &ClaimPair,
        relation: &str,
        rationale: &str,
        reviewer: &str,
        retry: bool,
    ) -> io::Result<()> {
        if !matches!(
            relation,
            "supports" | "contradicts" | "qualifies" | "unrelated" | "insufficient"
        ) || !crate::storage::valid_bounded_field(rationale, 500)
        {
            return Err(invalid("invalid research evidence relationship"));
        }
        self.connection.execute("INSERT INTO research_evidence_links(left_claim_id,right_claim_id,relation,rationale,reviewer_model_id,next_attempt_at_ms,created_at)
            VALUES (?1,?2,?3,?4,?5,?6,strftime('%Y-%m-%dT%H:%M:%fZ','now'))
            ON CONFLICT(left_claim_id,right_claim_id) DO UPDATE SET relation=excluded.relation,rationale=excluded.rationale,reviewer_model_id=excluded.reviewer_model_id,next_attempt_at_ms=excluded.next_attempt_at_ms",
            params![pair.left_id,pair.right_id,relation,rationale,reviewer,if retry {now_ms()?+300_000} else {0}]).map_err(database_error)?;
        Ok(())
    }

    pub fn record_research_claim_relation(
        &self,
        pair: &ClaimPair,
        relation: &str,
        rationale: &str,
        reviewer: &str,
    ) -> io::Result<()> {
        if !matches!(relation, "same_subject" | "related_facts")
            || !crate::storage::valid_bounded_field(rationale, 500)
            || !crate::storage::valid_bounded_field(reviewer, 200)
            || pair.left_id >= pair.right_id
        {
            return Err(invalid("invalid semantic claim relationship"));
        }
        self.connection.execute(
            "INSERT INTO research_claim_relations(left_claim_id,right_claim_id,relation,rationale,reviewer_model_id)
             VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(left_claim_id,right_claim_id) DO UPDATE SET
                relation=excluded.relation,rationale=excluded.rationale,reviewer_model_id=excluded.reviewer_model_id",
            params![pair.left_id,pair.right_id,relation,rationale,reviewer],
        ).map_err(database_error)?;
        Ok(())
    }

    pub fn research_basket_summary(&self) -> io::Result<serde_json::Value> {
        let count = |table: &str| {
            self.connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(database_error)
        };
        let sources = self.connection.query_row(
            "SELECT count(*),sum(harvest_cursor>=length(body)),sum(harvest_cursor<length(body)),
                sum(harvest_cursor<length(body) AND next_attempt_at_ms=9223372036854775807),
                sum(harvest_cursor<length(body) AND next_attempt_at_ms<=CAST(strftime('%s','now') AS INTEGER)*1000
                    AND claimed_until_ms<=CAST(strftime('%s','now') AS INTEGER)*1000),
                min(CASE WHEN harvest_cursor<length(body) AND next_attempt_at_ms<=CAST(strftime('%s','now') AS INTEGER)*1000
                    AND claimed_until_ms<=CAST(strftime('%s','now') AS INTEGER)*1000 THEN created_at END)
             FROM research_sources", [], |row| Ok(serde_json::json!({
                "total":row.get::<_,i64>(0)?,"completed":row.get::<_,Option<i64>>(1)?.unwrap_or_default(),
                "unfinished":row.get::<_,Option<i64>>(2)?.unwrap_or_default(),"parked":row.get::<_,Option<i64>>(3)?.unwrap_or_default(),
                "eligible":row.get::<_,Option<i64>>(4)?.unwrap_or_default(),"oldest_eligible_at":row.get::<_,Option<String>>(5)?
            })),
        ).map_err(database_error)?;
        let question_states = self.connection.query_row(
            "SELECT json_group_object(status,n) FROM (SELECT status,count(*) AS n FROM research_questions GROUP BY status)",
            [], |row| row.get::<_,Option<String>>(0),
        ).map_err(database_error)?.unwrap_or_else(|| "{}".into());
        let assertion_states = self.connection.query_row(
            "SELECT json_group_object(status,n) FROM (SELECT status,count(*) AS n FROM research_assertion_checks GROUP BY status)",
            [], |row| row.get::<_,Option<String>>(0),
        ).map_err(database_error)?.unwrap_or_else(|| "{}".into());
        let feedback_states = self
            .connection
            .query_row(
                "SELECT json_group_object(route || ':' || status,n),sum(n),min(oldest_at)
             FROM (SELECT route,status,count(*) AS n,min(created_at) AS oldest_at
                   FROM scholar_research_feedback WHERE status IN ('pending','queued','processing')
                   GROUP BY route,status)",
                [],
                |row| {
                    Ok(serde_json::json!({
                        "states":row.get::<_,Option<String>>(0)?.unwrap_or_else(|| "{}".into()),
                        "count":row.get::<_,Option<i64>>(1)?.unwrap_or_default(),
                        "oldest_at":row.get::<_,Option<String>>(2)?
                    }))
                },
            )
            .map_err(database_error)?;
        let assertion_admission = self
            .connection
            .query_row(
                "SELECT last_admission_reason FROM research_assertion_checks
             WHERE last_admission_reason IS NOT NULL ORDER BY updated_at DESC LIMIT 1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(database_error)?
            .flatten();
        let latest_assertion_selection = self
            .connection
            .query_row(
                "SELECT claim_id,status,selection_reason,derivation_depth FROM research_assertion_checks
                 ORDER BY updated_at DESC,claim_id DESC LIMIT 1",
                [],
                |row| {
                    Ok(serde_json::json!({
                        "claim_id":row.get::<_,i64>(0)?,
                        "status":row.get::<_,String>(1)?,
                        "reason":row.get::<_,String>(2)?,
                        "derivation_depth":row.get::<_,i64>(3)?
                    }))
                },
            )
            .optional()
            .map_err(database_error)?;
        Ok(serde_json::json!({
            "sources":sources,"unverified_claims":count("research_claims")?,
            "questions":count("research_questions")?,"question_states":serde_json::from_str::<serde_json::Value>(&question_states).unwrap_or_default(),
            "assertion_checks":count("research_assertion_checks")?,"assertion_states":serde_json::from_str::<serde_json::Value>(&assertion_states).unwrap_or_default(),
            "unresolved_feedback":feedback_states,
            "latest_assertion_admission_reason":assertion_admission,
            "latest_assertion_selection":latest_assertion_selection,
            "evidence_links":count("research_evidence_links")?,"semantic_relations":count("research_claim_relations")?
        }))
    }

    pub fn retained_claim_evidence(&self, id: i64) -> io::Result<serde_json::Value> {
        let claim = self.connection.query_row(
            "SELECT c.subject,c.relation,c.object,c.evidence_text,c.qualifications,c.extraction_confidence,s.url,s.id,c.evidence_byte_start,c.evidence_byte_end,s.content_hash
             FROM research_claims c JOIN research_sources s ON s.id=c.source_id WHERE c.id=?1",[id],
            |row|Ok(RetainedClaimEvidence {
                subject: row.get(0)?,
                relation: row.get(1)?,
                object: row.get(2)?,
                quotation: row.get(3)?,
                qualifications: row.get(4)?,
                confidence: row.get(5)?,
                source_url: row.get(6)?,
                source_id: row.get(7)?,
                byte_start: row.get(8)?,
                byte_end: row.get(9)?,
                content_hash: row.get(10)?,
            }),
        ).map_err(database_error)?;
        let mut statement = self.connection.prepare("SELECT l.relation,l.rationale,s.url,l.reviewer_model_id,1 FROM research_evidence_links l
            JOIN research_claims other ON other.id=CASE WHEN l.left_claim_id=?1 THEN l.right_claim_id ELSE l.left_claim_id END
            JOIN research_sources s ON s.id=other.source_id WHERE (l.left_claim_id=?1 OR l.right_claim_id=?1) AND l.next_attempt_at_ms=0
            UNION ALL
            SELECT r.relation,r.rationale,s.url,r.reviewer_model_id,0 FROM research_claim_relations r
            JOIN research_claims other ON other.id=CASE WHEN r.left_claim_id=?1 THEN r.right_claim_id ELSE r.left_claim_id END
            JOIN research_sources s ON s.id=other.source_id WHERE r.left_claim_id=?1 OR r.right_claim_id=?1
            ORDER BY 5 DESC").map_err(database_error)?;
        let links = statement
            .query_map([id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)? != 0,
                ))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        let group = |url: &str| {
            url::Url::parse(url)
                .ok()
                .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
                .unwrap_or_else(|| url.to_owned())
        };
        let mut groups = std::collections::HashMap::<String, u8>::new();
        groups.insert(group(&claim.source_url), 1);
        let mut relationships = Vec::new();
        for (kind, rationale, url, reviewer, counts_toward_host_groups) in links {
            if counts_toward_host_groups {
                let bit = match kind.as_str() {
                    "supports" => 1,
                    "contradicts" => 2,
                    "qualifies" => 4,
                    _ => 0,
                };
                if bit > 0 {
                    *groups.entry(group(&url)).or_default() |= bit;
                }
            }
            relationships.push(serde_json::json!({"relation":kind,"rationale":rationale,"source_url":url,"reviewer_model_id":reviewer}));
        }
        let supporting = groups
            .values()
            .filter(|value| **value & 1 != 0 && **value & 2 == 0)
            .count();
        let contradicting = groups
            .values()
            .filter(|value| **value & 2 != 0 && **value & 1 == 0)
            .count();
        let disputed = groups.values().filter(|value| **value & 3 == 3).count();
        let qualified = groups.values().filter(|value| **value & 4 != 0).count();
        let support_score =
            supporting as f64 / (supporting + contradicting + disputed + qualified + 2) as f64;
        Ok(
            serde_json::json!({"id":id,"subject":claim.subject,"relation":claim.relation,"object":claim.object,"evidence_text":claim.quotation,
            "qualifications":claim.qualifications,"extraction_confidence":claim.confidence,"source_url":claim.source_url,
            "status":"unverified","source_archive_id":claim.source_id,"source_content_hash":claim.content_hash,
            "evidence_byte_start":claim.byte_start,"evidence_byte_end":claim.byte_end,
            "supporting_host_groups":supporting,"contradicting_host_groups":contradicting,
            "disputed_host_groups":disputed,"qualifying_host_groups":qualified,"provisional_support_score":support_score,
            "independence_verified":false,"relationships":relationships}),
        )
    }

    pub fn research_basket_items(&self, kind: &str) -> io::Result<Vec<serde_json::Value>> {
        let sql = match kind {
            "claims" => {
                "SELECT json_object('id',c.id,'subject',c.subject,'relation',c.relation,'object',c.object,'extraction_confidence',c.extraction_confidence,'source_id',c.source_id,'status','unverified') FROM research_claims c ORDER BY c.id DESC LIMIT 50"
            }
            "questions" => {
                "SELECT json_object('id',id,'question',question,'source_id',source_id,'depth',depth,'status',status,'scholar_job_id',scholar_job_id) FROM research_questions ORDER BY id DESC LIMIT 50"
            }
            "sources" => {
                "SELECT json_object('id',id,'title',title,'url',url,'content_hash',content_hash,'provider',provider,'query',query,'published_at',published_at,'source_scope',source_scope,'capture_status',capture_status,'input_characters',input_characters,'retained_characters',retained_characters,'truncated',truncated,'retrieved_at',created_at,'harvest_cursor',harvest_cursor,'next_attempt_at_ms',next_attempt_at_ms,'last_error',last_error) FROM research_sources ORDER BY id DESC LIMIT 50"
            }
            _ => return Err(invalid("basket must be claims, questions, or sources")),
        };
        let mut statement = self.connection.prepare(sql).map_err(database_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        rows.into_iter()
            .map(|raw| serde_json::from_str(&raw).map_err(invalid_json))
            .collect()
    }

    pub fn retained_source_detail(&self, id: i64) -> io::Result<serde_json::Value> {
        let raw: String = self.connection.query_row("SELECT json_object('id',id,'job_id',job_id,'title',title,'url',url,'content_hash',content_hash,'provider',provider,'query',query,'published_at',published_at,'source_scope',source_scope,'capture_status',capture_status,'input_characters',input_characters,'retained_characters',retained_characters,'truncated',truncated,'retrieved_at',created_at,'body',body,'harvest_cursor',harvest_cursor,'last_error',last_error) FROM research_sources WHERE id=?1",[id],|row|row.get(0)).map_err(database_error)?;
        serde_json::from_str(&raw).map_err(invalid_json)
    }

    pub fn retry_retained_source(&self, id: i64) -> io::Result<()> {
        let changed = self.connection.execute("UPDATE research_sources SET harvest_cursor=0,next_attempt_at_ms=0,last_error=NULL WHERE id=?1 AND claimed_until_ms<=?2",params![id,now_ms()?]).map_err(database_error)?;
        if changed != 1 {
            return Err(invalid(
                "source does not exist or is currently being harvested",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retained_claim(
        subject: impl Into<String>,
        relation: impl Into<String>,
        object: impl Into<String>,
        evidence_text: impl Into<String>,
        extraction_confidence: f64,
    ) -> RetainedClaim {
        RetainedClaim {
            subject: subject.into(),
            relation: relation.into(),
            object: object.into(),
            evidence_text: evidence_text.into(),
            qualifications: String::new(),
            extraction_confidence,
        }
    }

    fn retained_question(
        question: impl Into<String>,
        evidence_text: impl Into<String>,
    ) -> RetainedQuestion {
        RetainedQuestion {
            question: question.into(),
            evidence_text: evidence_text.into(),
        }
    }

    fn claim_pair_ids(left_id: i64, right_id: i64) -> ClaimPair {
        ClaimPair {
            left_id,
            right_id,
            left: serde_json::Value::Null,
            right: serde_json::Value::Null,
        }
    }

    #[test]
    fn research_source_capture_status_preserves_scope_and_truncation() {
        assert_eq!(
            super::super::research_source_capture_status("full_page", false),
            "complete"
        );
        assert_eq!(
            super::super::research_source_capture_status("selected_section", false),
            "excerpted"
        );
        assert_eq!(
            super::super::research_source_capture_status("finding_source_excerpt", false),
            "excerpted"
        );
        assert_eq!(
            super::super::research_source_capture_status("full_page", true),
            "truncated"
        );
    }

    fn first_research_claim_span(store: &LearningStore) -> (i64, i64) {
        store
            .connection
            .query_row(
                "SELECT evidence_byte_start,evidence_byte_end FROM research_claims",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    }

    #[test]
    fn existing_excerpts_import_without_refetching_or_changing_review_status() {
        let mut store = crate::test_support::in_memory_learning_store();
        let plan = crate::research::ResearchPlan {
            question: "Where is the Amazon rainforest?".into(),
            queries: vec!["Amazon rainforest".into()],
        };
        store.enqueue_research_plan("import", 1, &plan).unwrap();
        let job = store
            .claim_conversation_research("import", 1)
            .unwrap()
            .unwrap();
        store.connection.execute("INSERT INTO scholar_findings(job_id,source_title,source_url,source_excerpt,subject,relation,object,evidence_text,confidence,status)
            VALUES (?1,'Amazon','https://example.org/page','The Amazon rainforest is in South America.','Amazon rainforest','is in','South America','The Amazon rainforest is in South America.',0.98,'rejected')",[&job.id]).unwrap();
        assert_eq!(store.bootstrap_research_sources().unwrap(), 1);
        assert_eq!(store.bootstrap_research_sources().unwrap(), 0);
        assert_eq!(store.import_retained_research_sources().unwrap(), 0);
        assert_eq!(
            store.scholar_findings(&job.id).unwrap()[0].status,
            "rejected"
        );
        assert!(store.active_semantic_claims().unwrap().is_empty());
        let requests: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM host_request_log", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(requests, 0);
    }

    #[test]
    fn source_archive_records_truncation_unicode_counts_and_full_input_hash() {
        let store = crate::test_support::in_memory_learning_store();
        let plan = crate::research::ResearchPlan {
            question: "Describe the article".into(),
            queries: vec!["article".into()],
        };
        store.enqueue_research_plan("archive", 1, &plan).unwrap();
        let job: String = store
            .connection
            .query_row("SELECT id FROM scholar_jobs LIMIT 1", [], |row| row.get(0))
            .unwrap();
        let body = format!("{}tail-one", "🌱".repeat(MAX_RETAINED_SOURCE_CHARS));
        store
            .retain_research_source_with_metadata(super::super::ResearchSourceInput {
                job_id: &job,
                title: "Article",
                url: "https://example.org/article",
                body: &body,
                provider: "wikipedia",
                source_scope: "selected_section",
                query: Some("article query"),
                published_at: Some("2026-09-30T00:00:00Z"),
            })
            .unwrap();
        let source_id: i64 = store
            .connection
            .query_row("SELECT id FROM research_sources", [], |row| row.get(0))
            .unwrap();
        let details = store.retained_source_detail(source_id).unwrap();
        assert_eq!(details["provider"], "wikipedia");
        assert_eq!(details["query"], "article query");
        assert_eq!(details["published_at"], "2026-09-30T00:00:00Z");
        assert_eq!(details["source_scope"], "selected_section");
        assert_eq!(details["capture_status"], "truncated");
        assert_eq!(details["input_characters"], MAX_RETAINED_SOURCE_CHARS + 8);
        assert_eq!(details["retained_characters"], MAX_RETAINED_SOURCE_CHARS);
        assert_eq!(
            details["body"].as_str().unwrap().chars().count(),
            MAX_RETAINED_SOURCE_CHARS
        );
        let first_hash = details["content_hash"].as_str().unwrap().to_owned();

        let changed_tail = format!("{}tail-two", "🌱".repeat(MAX_RETAINED_SOURCE_CHARS));
        store
            .retain_research_source_with_metadata(super::super::ResearchSourceInput {
                job_id: &job,
                title: "Article",
                url: "https://example.org/article",
                body: &changed_tail,
                provider: "wikipedia",
                source_scope: "selected_section",
                query: Some("article query"),
                published_at: Some("2026-09-30T00:00:00Z"),
            })
            .unwrap();
        let versions: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM research_sources", [], |row| {
                row.get(0)
            })
            .unwrap();
        let second_hash: String = store
            .connection
            .query_row(
                "SELECT content_hash FROM research_sources WHERE id != ?1",
                [source_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(versions, 2);
        assert_ne!(first_hash, second_hash);
    }

    fn source(store: &mut LearningStore, url: &str, passage: &str) -> ResearchSource {
        let plan = crate::research::ResearchPlan {
            question: "Where is the Amazon rainforest?".into(),
            queries: vec!["Amazon rainforest".into()],
        };
        store.enqueue_research_plan("baskets", 1, &plan).unwrap();
        let job: String = store
            .connection
            .query_row("SELECT id FROM scholar_jobs LIMIT 1", [], |row| row.get(0))
            .unwrap();
        store
            .retain_research_source(&job, "Amazon rainforest", url, passage)
            .unwrap();
        store.claim_research_source().unwrap().unwrap()
    }

    #[test]
    fn scout_background_stage_cursor_survives_calls_and_cycles_boundedly() {
        let mut store = crate::test_support::in_memory_learning_store();
        let stages = (0..7)
            .map(|_| store.claim_next_scout_stage().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(stages, [0, 1, 2, 3, 4, 0, 1]);
    }

    #[test]
    fn selected_retained_assertion_queues_provisional_scholar_work() {
        let mut store = crate::test_support::in_memory_learning_store();
        let passage = "The observatory opened in 1974.";
        let source = source(&mut store, "https://example.org/observatory", passage);
        let baskets = ResearchBaskets {
            claims: vec![retained_claim(
                "The observatory",
                "opened in",
                "1974",
                passage,
                0.94,
            )],
            questions: vec![],
        };
        store.finish_research_baskets(&source, &baskets).unwrap();
        let lead = store.next_research_assertion_lead().unwrap().unwrap();
        assert_eq!(lead.source_url, "https://example.org/observatory");
        assert_eq!(lead.content_hash.len(), 64);
        let question = "What evidence establishes when the observatory opened?";
        store
            .record_research_assertion_selection(
                &lead,
                Some(question),
                "A second source can verify this dated public claim.",
            )
            .unwrap();
        assert!(store.has_pending_research_assertion_lead().unwrap());
        let (claim_id, job_id) = store.queue_research_assertion_lead().unwrap().unwrap();
        assert_eq!(claim_id, lead.claim_id);
        let investigation = store.scout_investigations().unwrap().remove(0);
        assert_eq!(investigation.trigger_kind, "assertion");
        assert_eq!(
            investigation.trigger_ref,
            format!("research_claim:{claim_id}")
        );
        assert_eq!(investigation.scholar_job_id, job_id);
        assert_eq!(investigation.question, question);
        assert!(store.active_semantic_claims().unwrap().is_empty());
        assert_eq!(
            store.research_assertion_selection(claim_id).unwrap(),
            "queued"
        );
        store
            .connection
            .execute(
                "UPDATE scholar_jobs SET status='needs_review' WHERE id=?1",
                [&job_id],
            )
            .unwrap();
        assert!(store.queue_research_assertion_lead().unwrap().is_none());
        assert_eq!(
            store.research_assertion_selection(claim_id).unwrap(),
            "proposed"
        );
        store
            .connection
            .execute(
                "UPDATE scholar_jobs SET status='promoted' WHERE id=?1",
                [&job_id],
            )
            .unwrap();
        assert!(store.queue_research_assertion_lead().unwrap().is_none());
        assert_eq!(
            store.research_assertion_selection(claim_id).unwrap(),
            "verified"
        );
    }

    #[test]
    fn skipped_retained_assertions_are_recorded_without_creating_research_jobs() {
        let mut store = crate::test_support::in_memory_learning_store();
        let passage = "A public page mentions an unrelated detail.";
        let source = source(&mut store, "https://example.org/detail", passage);
        store
            .finish_research_baskets(
                &source,
                &ResearchBaskets {
                    claims: vec![retained_claim(
                        "The page",
                        "mentions",
                        "an unrelated detail",
                        passage,
                        0.8,
                    )],
                    questions: vec![],
                },
            )
            .unwrap();
        let lead = store.next_research_assertion_lead().unwrap().unwrap();
        store
            .record_research_assertion_selection(
                &lead,
                None,
                "Checking this detail adds no useful public knowledge.",
            )
            .unwrap();
        assert!(store.next_research_assertion_lead().unwrap().is_none());
        assert!(!store.has_pending_research_assertion_lead().unwrap());
        assert!(store.scout_investigations().unwrap().is_empty());
    }

    #[test]
    fn exhausted_source_is_preserved_and_only_resumes_after_explicit_retry() {
        let mut store = crate::test_support::in_memory_learning_store();
        let passage = "The observatory opened in 1974.";
        let source = source(&mut store, "https://example.org/observatory", passage);
        store
            .park_research_source(source.id, "invalid questions field")
            .unwrap();
        assert!(store.claim_research_source().unwrap().is_none());
        let detail = store.retained_source_detail(source.id).unwrap();
        assert_eq!(detail["body"], passage);
        assert_eq!(detail["harvest_cursor"], source.cursor);
        assert_eq!(detail["last_error"], "invalid questions field");
        store.retry_retained_source(source.id).unwrap();
        let resumed = store.claim_research_source().unwrap().unwrap();
        assert_eq!(resumed.id, source.id);
        assert_eq!(resumed.passage, passage);
    }

    #[test]
    fn fact_shaped_false_assertions_are_retained_without_promotion() {
        let mut store = crate::test_support::in_memory_learning_store();
        let source = source(
            &mut store,
            "https://example.org/article",
            "The Amazon rainforest is made of chocolate.",
        );
        let baskets = ResearchBaskets {
            claims: vec![retained_claim(
                "Amazon rainforest",
                "is made of",
                "chocolate",
                source.passage.clone(),
                0.9,
            )],
            questions: vec![retained_question(
                "What materials form the Amazon rainforest?",
                source.passage.clone(),
            )],
        };
        assert_eq!(
            store.finish_research_baskets(&source, &baskets).unwrap(),
            (1, 1)
        );
        assert!(store.active_semantic_claims().unwrap().is_empty());
        assert!(
            store
                .scholar_findings(
                    &store
                        .connection
                        .query_row::<String, _, _>(
                            "SELECT id FROM scholar_jobs LIMIT 1",
                            [],
                            |row| row.get(0)
                        )
                        .unwrap()
                )
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn harvested_claim_and_question_offsets_point_to_exact_unicode_source_bytes() {
        let mut store = crate::test_support::in_memory_learning_store();
        let quote = "The Amazon rainforest is made of chocolate.";
        let body = format!("🌱🌱\n{quote}\nA second sentence.");
        let source = source(&mut store, "https://example.org/unicode", &body);
        let baskets = ResearchBaskets {
            claims: vec![retained_claim(
                "Amazon rainforest",
                "is made of",
                "chocolate",
                quote,
                0.9,
            )],
            questions: vec![retained_question("What material is named?", quote)],
        };
        store.finish_research_baskets(&source, &baskets).unwrap();
        let expected_start = i64::try_from("🌱🌱\n".len()).unwrap();
        let (claim_start, claim_end) = first_research_claim_span(&store);
        let (question_start, question_end): (i64, i64) = store
            .connection
            .query_row(
                "SELECT evidence_byte_start,evidence_byte_end FROM research_question_sources",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(claim_start, expected_start);
        assert_eq!((claim_start, claim_end), (question_start, question_end));
        let detail = store.retained_source_detail(source.id).unwrap();
        let retained = detail["body"].as_str().unwrap().as_bytes();
        assert_eq!(
            std::str::from_utf8(&retained[claim_start as usize..claim_end as usize]).unwrap(),
            quote
        );
    }

    #[test]
    fn harvested_evidence_offsets_survive_a_source_chunk_boundary() {
        let mut store = crate::test_support::in_memory_learning_store();
        let fact = "The Amazon rainforest is in South America.";
        let body = format!("{}{fact}\nMore source text.", "é".repeat(1_990));
        let source = source(&mut store, "https://example.org/chunked", &body);
        assert!(!source.passage.contains(fact));
        store
            .finish_research_baskets(&source, &ResearchBaskets::default())
            .unwrap();
        let next = store.claim_research_source().unwrap().unwrap();
        assert!(next.passage.contains(fact));
        let baskets = ResearchBaskets {
            claims: vec![retained_claim(
                "Amazon rainforest",
                "is in",
                "South America",
                fact,
                0.95,
            )],
            questions: vec![],
        };
        store.finish_research_baskets(&next, &baskets).unwrap();
        let (start, end) = first_research_claim_span(&store);
        assert_eq!(
            &body.as_bytes()[start as usize..end as usize],
            fact.as_bytes()
        );
    }

    #[test]
    fn invalid_item_is_skipped_without_losing_valid_items_or_blocking_the_cursor() {
        let mut store = crate::test_support::in_memory_learning_store();
        let source = source(
            &mut store,
            "https://example.org/article",
            "The Amazon rainforest is in South America.",
        );
        let baskets = ResearchBaskets {
            claims: vec![retained_claim(
                "Amazon rainforest",
                "is in",
                "South America",
                source.passage.clone(),
                0.9,
            )],
            questions: vec![retained_question("Why is it diverse?", "invented evidence")],
        };
        assert_eq!(
            store.finish_research_baskets(&source, &baskets).unwrap(),
            (1, 0)
        );
        assert_eq!(
            store.research_basket_summary().unwrap()["unverified_claims"],
            1
        );
        assert_eq!(store.research_basket_summary().unwrap()["questions"], 0);
        let cursor: i64 = store
            .connection
            .query_row(
                "SELECT harvest_cursor FROM research_sources WHERE id=?1",
                [source.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cursor, source.next_cursor);
    }

    #[test]
    fn retained_content_is_idempotent_and_processed_in_bounded_chunks() {
        let mut store = crate::test_support::in_memory_learning_store();
        let passage = "A sourced assertion.\n".repeat(200);
        let first = source(&mut store, "https://example.org/article", &passage);
        assert!(first.passage.chars().count() <= RESEARCH_HARVEST_CHUNK_CHARS);
        assert!(store.claim_research_source().unwrap().is_none());
        store
            .finish_research_baskets(&first, &ResearchBaskets::default())
            .unwrap();
        let second = store.claim_research_source().unwrap().unwrap();
        assert_eq!(second.cursor, first.next_cursor);
        assert!(second.next_cursor > first.next_cursor);
    }

    #[test]
    fn retained_questions_deduplicate_and_respect_user_priority() {
        let mut store = crate::test_support::in_memory_learning_store();
        let text = "The Amazon rainforest is in South America.";
        let first = source(&mut store, "https://one.example/article", text);
        let baskets = ResearchBaskets {
            claims: vec![],
            questions: vec![retained_question(
                "Why is the Amazon rainforest biodiverse?",
                text,
            )],
        };
        store.finish_research_baskets(&first, &baskets).unwrap();
        let second = source(&mut store, "https://two.example/article", text);
        assert_eq!(
            store.finish_research_baskets(&second, &baskets).unwrap(),
            (0, 0)
        );
        assert!(!store.queue_retained_question().unwrap());
        assert_eq!(store.research_basket_summary().unwrap()["questions"], 1);
        let provenance: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM research_question_sources",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(provenance, 2);
        let question_id: i64 = store
            .connection
            .query_row("SELECT id FROM research_questions", [], |row| row.get(0))
            .unwrap();
        let key = format!("source-question:{question_id}");
        store
            .connection
            .execute("UPDATE scholar_jobs SET status='no_result'", [])
            .unwrap();
        store.connection.execute("INSERT INTO scout_investigations(trigger_kind,topic_key,trigger_ref,question,scholar_job_id,selection_reason,created_at)
            SELECT 'expansion',?1,'source:fixture','fixture',id,'fixture',strftime('%Y-%m-%dT%H:%M:%fZ','now','-8 days') FROM scholar_jobs LIMIT 1",[&key]).unwrap();
        assert!(store.queue_retained_question().unwrap());
        assert_eq!(
            store.research_basket_items("questions").unwrap()[0]["status"],
            "queued"
        );
    }

    #[test]
    fn blocked_retained_questions_stay_pending_for_interval_daily_and_duplicate_gates() {
        for blocker in ["interval", "daily", "duplicate"] {
            let mut store = crate::test_support::in_memory_learning_store();
            let text = "The Amazon rainforest is in South America.";
            let source = source(
                &mut store,
                &format!("https://{blocker}.example/amazon"),
                text,
            );
            let question = "What evidence describes Amazon rainforest biodiversity?";
            store
                .finish_research_baskets(
                    &source,
                    &ResearchBaskets {
                        claims: vec![],
                        questions: vec![retained_question(question, text)],
                    },
                )
                .unwrap();
            store
                .connection
                .execute("UPDATE scholar_jobs SET status='no_result'", [])
                .unwrap();

            let blocking_question = if blocker == "duplicate" {
                question
            } else {
                "How does the Amazon basin receive rainfall?"
            };
            assert!(
                store
                    .enqueue_scout_research_with_reason(
                        "sparse",
                        &format!("blocker:{blocker}"),
                        "fixture",
                        blocking_question,
                        "Fixture investigation for admission coverage",
                    )
                    .unwrap()
            );
            if blocker != "interval" {
                store
                    .connection
                    .execute(
                        "UPDATE scout_investigations SET created_at=strftime('%Y-%m-%dT%H:%M:%fZ','now','-6 minutes')",
                        [],
                    )
                    .unwrap();
            }
            if blocker == "daily" {
                let blocking_job: String = store
                    .connection
                    .query_row(
                        "SELECT scholar_job_id FROM scout_investigations LIMIT 1",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                for index in 1..20 {
                    store.connection.execute(
                        "INSERT INTO scout_investigations(trigger_kind,topic_key,trigger_ref,question,scholar_job_id,selection_reason,created_at)
                         VALUES('stale',?1,'fixture','A dated status check?',?2,'daily budget fixture',strftime('%Y-%m-%dT%H:%M:%fZ','now','-6 minutes'))",
                        rusqlite::params![format!("daily:{index}"),blocking_job],
                    ).unwrap();
                }
            }

            let expected = match blocker {
                "interval" => "Scout minimum investigation interval has not elapsed",
                "daily" => "Scout daily investigation budget exhausted",
                _ => "Same question is pending or was researched within the last day",
            };
            match store
                .try_enqueue_scout_research(
                    "uncertain",
                    &format!("assertion:{blocker}"),
                    "fixture",
                    question,
                    "Admission blocker fixture",
                )
                .unwrap()
            {
                ScoutAdmission::Deferred(reason) | ScoutAdmission::Blocked(reason) => {
                    assert_eq!(reason, expected, "blocker={blocker}");
                }
                ScoutAdmission::Queued(_) => panic!("blocked admission unexpectedly queued"),
            }
            assert!(
                !store.queue_retained_question().unwrap(),
                "blocker={blocker}"
            );
            assert_eq!(
                store.research_basket_items("questions").unwrap()[0]["status"],
                "pending",
                "blocker={blocker}"
            );
        }
    }

    #[test]
    fn research_priority_is_conversation_then_source_question_then_scout() {
        let mut store = crate::test_support::in_memory_learning_store();
        let text = "The Amazon rainforest is in South America.";
        let source = source(&mut store, "https://example.org/amazon", text);
        store
            .finish_research_baskets(
                &source,
                &ResearchBaskets {
                    claims: vec![],
                    questions: vec![retained_question(
                        "Why is the Amazon rainforest biodiverse?",
                        text,
                    )],
                },
            )
            .unwrap();
        store
            .connection
            .execute("UPDATE scholar_jobs SET status='no_result'", [])
            .unwrap();
        let question_id: i64 = store
            .connection
            .query_row("SELECT id FROM research_questions", [], |row| row.get(0))
            .unwrap();
        let topic_key = format!("source-question:{question_id}");
        assert!(
            store
                .enqueue_scout_research_with_reason(
                    "expansion",
                    &topic_key,
                    "source:1",
                    "Why is the Amazon rainforest biodiverse?",
                    "Retained source passage raises a grounded follow-up question",
                )
                .unwrap()
        );
        store
            .connection
            .execute(
                "UPDATE research_questions SET status='queued',scholar_job_id=(SELECT scholar_job_id FROM scout_investigations WHERE topic_key=?1)",
                [&topic_key],
            )
            .unwrap();
        store.connection.execute("UPDATE scout_investigations SET created_at=strftime('%Y-%m-%dT%H:%M:%fZ','now','-8 days')", []).unwrap();
        store
            .enqueue_scout_research_with_reason(
                "model_probe",
                "mars moons",
                "model_probe:1",
                "What moons orbit Mars?",
                "Scout generated question",
            )
            .unwrap();
        store
            .enqueue_research("frontend", 1, "What flavours of crisps exist?")
            .unwrap();

        assert_eq!(
            store.claim_scholar_job().unwrap().unwrap().question,
            "What flavours of crisps exist?"
        );
        assert_eq!(
            store.claim_scholar_job().unwrap().unwrap().question,
            "Why is the Amazon rainforest biodiverse?"
        );
        assert_eq!(
            store.claim_scholar_job().unwrap().unwrap().question,
            "What moons orbit Mars?"
        );
        assert!(store.claim_scholar_job().unwrap().is_none());
    }

    #[test]
    fn overlapping_subjects_are_review_candidates_without_merging_claims() {
        let mut store = crate::test_support::in_memory_learning_store();
        for (passage, subject, relation, object) in [
            (
                "The Amazon rainforest is in South America.",
                "Amazon rainforest",
                "is in",
                "South America",
            ),
            (
                "Amazon rainforest is located in South America.",
                "Rainforest of Amazon",
                "is located in",
                "South America",
            ),
        ] {
            let source = source(&mut store, "https://example.org/amazon", passage);
            store
                .finish_research_baskets(
                    &source,
                    &ResearchBaskets {
                        claims: vec![retained_claim(subject, relation, object, passage, 0.9)],
                        questions: vec![],
                    },
                )
                .unwrap();
        }
        let pair = store.next_research_claim_pair().unwrap().unwrap();
        assert_eq!(pair.left_id, 1);
        assert_eq!(pair.right_id, 2);
        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM research_claims", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn source_asserted_aliases_create_candidates_without_auto_merging_entities() {
        let mut store = crate::test_support::in_memory_learning_store();
        for (passage, subject, relation, object) in [
            (
                "Clark Kent is also known as Superman.",
                "Clark Kent",
                "is also known as",
                "Superman",
            ),
            (
                "Superman works as a reporter.",
                "Superman",
                "works as",
                "a reporter",
            ),
        ] {
            let source = source(&mut store, "https://example.org/comics", passage);
            store
                .finish_research_baskets(
                    &source,
                    &ResearchBaskets {
                        claims: vec![retained_claim(subject, relation, object, passage, 0.9)],
                        questions: vec![],
                    },
                )
                .unwrap();
        }

        let pair = store.next_research_claim_pair().unwrap().unwrap();
        assert_eq!(pair.left_id, 1);
        assert_eq!(pair.right_id, 2);
        assert_eq!(
            store.research_basket_summary().unwrap()["unverified_claims"],
            2
        );
        assert_eq!(
            store.research_basket_summary().unwrap()["semantic_relations"],
            0
        );
    }

    #[test]
    fn support_counts_hosts_once_and_contradictions_lower_the_score() {
        let mut store = crate::test_support::in_memory_learning_store();
        let mut ids = Vec::new();
        for (url, object) in [
            ("https://one.example/a", "South America"),
            ("https://one.example/b", "South America"),
            ("https://two.example/a", "South America"),
            ("https://three.example/a", "Africa"),
        ] {
            let text = format!("The Amazon rainforest is in {object}.");
            let source = source(&mut store, url, &text);
            let baskets = ResearchBaskets {
                claims: vec![retained_claim(
                    "Amazon rainforest",
                    "is in",
                    object,
                    text,
                    0.98,
                )],
                questions: vec![],
            };
            store.finish_research_baskets(&source, &baskets).unwrap();
            let id: i64 = store
                .connection
                .query_row(
                    "SELECT id FROM research_claims WHERE source_id=?1",
                    [source.id],
                    |row| row.get(0),
                )
                .unwrap();
            ids.push(id);
        }
        for other in [ids[1], ids[2]] {
            let pair = claim_pair_ids(ids[0], other);
            store
                .record_research_evidence_link(
                    &pair,
                    "supports",
                    "The quotations express the same location.",
                    "mock-reviewer",
                    false,
                )
                .unwrap();
        }
        let before = store.retained_claim_evidence(ids[0]).unwrap();
        assert_eq!(before["supporting_host_groups"], 2);
        store
            .record_research_claim_relation(
                &claim_pair_ids(ids[0], ids[2]),
                "same_subject",
                "Both claims identify the Amazon rainforest.",
                "mock-reviewer",
            )
            .unwrap();
        let with_semantic_edge = store.retained_claim_evidence(ids[0]).unwrap();
        assert_eq!(with_semantic_edge["supporting_host_groups"], 2);
        assert!(
            with_semantic_edge["relationships"]
                .as_array()
                .unwrap()
                .iter()
                .any(|edge| edge["relation"] == "same_subject")
        );
        let pair = claim_pair_ids(ids[0], ids[3]);
        store
            .record_research_evidence_link(
                &pair,
                "contradicts",
                "The same rainforest is placed on a different continent.",
                "mock-reviewer",
                false,
            )
            .unwrap();
        let after = store.retained_claim_evidence(ids[0]).unwrap();
        assert_eq!(after["contradicting_host_groups"], 1);
        assert!(
            after["provisional_support_score"].as_f64().unwrap()
                < before["provisional_support_score"].as_f64().unwrap()
        );
        assert_eq!(after["independence_verified"], false);
        assert!(store.active_semantic_claims().unwrap().is_empty());
    }

    #[test]
    fn assertion_derived_research_is_bounded_to_three_generations() {
        let mut store = crate::test_support::in_memory_learning_store();
        let passage = "The observatory opened in 1974.";
        let source = source(&mut store, "https://example.org/original", passage);
        store
            .finish_research_baskets(
                &source,
                &ResearchBaskets {
                    claims: vec![retained_claim(
                        "The observatory",
                        "opened in",
                        "1974",
                        passage,
                        0.95,
                    )],
                    questions: vec![],
                },
            )
            .unwrap();

        for expected_depth in 0..3 {
            let lead = store.next_research_assertion_lead().unwrap().unwrap();
            assert_eq!(lead.derivation_depth, expected_depth);
            let question = format!("What independent source checks generation {expected_depth}?");
            store
                .record_research_assertion_selection(
                    &lead,
                    Some(&question),
                    "A bounded check could confirm this provisional assertion.",
                )
                .unwrap();
            let (claim_id, job_id) = store.queue_research_assertion_lead().unwrap().unwrap();
            assert_eq!(claim_id, lead.claim_id);
            store
                .connection
                .execute(
                    "UPDATE scout_investigations SET created_at=strftime('%Y-%m-%dT%H:%M:%fZ','now','-10 minutes') WHERE scholar_job_id=?1",
                    [&job_id],
                )
                .unwrap();

            let derived_passage = format!("The observatory recorded generation {expected_depth}.");
            store
                .retain_research_source(
                    &job_id,
                    "Derived observatory evidence",
                    &format!("https://example.org/derived/{expected_depth}"),
                    &derived_passage,
                )
                .unwrap();
            let derived_source = store.claim_research_source().unwrap().unwrap();
            store
                .finish_research_baskets(
                    &derived_source,
                    &ResearchBaskets {
                        claims: vec![retained_claim(
                            "The observatory",
                            "recorded generation",
                            expected_depth.to_string(),
                            &derived_passage,
                            0.92,
                        )],
                        questions: vec![],
                    },
                )
                .unwrap();
        }

        assert!(store.next_research_assertion_lead().unwrap().is_none());
        let (status, depth, reason): (String, i64, String) = store
            .connection
            .query_row(
                "SELECT status,derivation_depth,selection_reason FROM research_assertion_checks ORDER BY derivation_depth DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "skipped");
        assert_eq!(depth, 3);
        assert!(reason.contains("maximum assertion derivation depth"));
    }

    #[test]
    fn quota_deferred_assertion_reuses_its_selected_question_on_later_admission() {
        let mut store = crate::test_support::in_memory_learning_store();
        let passage = "The observatory opened in 1974.";
        let source = source(&mut store, "https://example.org/observatory", passage);
        store
            .finish_research_baskets(
                &source,
                &ResearchBaskets {
                    claims: vec![retained_claim(
                        "The observatory",
                        "opened in",
                        "1974",
                        passage,
                        0.95,
                    )],
                    questions: vec![],
                },
            )
            .unwrap();
        let lead = store.next_research_assertion_lead().unwrap().unwrap();
        let selected_question = "What independent record confirms the opening year?";
        let selected_reason = "The dated public statement can be checked against another source.";
        store
            .record_research_assertion_selection(&lead, Some(selected_question), selected_reason)
            .unwrap();
        assert!(
            store
                .enqueue_scout_research_with_reason(
                    "stale",
                    "fixture:blocking-investigation",
                    "fixture",
                    "What rainfall reaches the observatory site?",
                    "Temporary spacing fixture",
                )
                .unwrap()
        );
        assert!(store.queue_research_assertion_lead().unwrap().is_none());
        assert_eq!(
            store.research_assertion_selection(lead.claim_id).unwrap(),
            "pending"
        );
        let deferred_reason: String = store
            .connection
            .query_row(
                "SELECT last_admission_reason FROM research_assertion_checks WHERE claim_id=?1",
                [lead.claim_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(deferred_reason.contains("minimum investigation interval"));
        store.connection.execute(
            "UPDATE scout_investigations SET created_at=strftime('%Y-%m-%dT%H:%M:%fZ','now','-6 minutes')",
            [],
        ).unwrap();
        let queued = store.queue_research_assertion_lead().unwrap().unwrap();
        assert_eq!(queued.0, lead.claim_id);
        assert_eq!(
            store.research_assertion_selection(lead.claim_id).unwrap(),
            "queued"
        );
        let stored: (String, String) = store
            .connection
            .query_row(
                "SELECT question,selection_reason FROM research_assertion_checks WHERE claim_id=?1",
                [lead.claim_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(stored, (selected_question.into(), selected_reason.into()));
    }
}
