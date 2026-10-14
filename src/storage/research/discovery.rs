//! Persisted search candidates, extraction attempts and provenance review jobs.
use super::*;

impl LearningStore {
    pub fn record_typed_search_candidate(
        &self,
        job_id: &str,
        provider: &str,
        query: &str,
        result_type: &str,
        title: &str,
        url: &str,
        snippet: &str,
        structured_data_json: &str,
    ) -> io::Result<()> {
        if structured_data_json.len() > 8_000
            || serde_json::from_str::<serde_json::Value>(structured_data_json).is_err()
        {
            return Err(invalid("candidate structured data must be valid JSON"));
        }
        let inserted = self
            .connection
            .execute(
                "INSERT OR IGNORE INTO scholar_search_candidates
                (job_id, provider, query, result_type, title, url, snippet,
                 structured_data_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![
                    job_id,
                    provider,
                    query.chars().take(1_000).collect::<String>(),
                    result_type,
                    title.chars().take(500).collect::<String>(),
                    url,
                    snippet.chars().take(2_000).collect::<String>(),
                    structured_data_json,
                ],
            )
            .map_err(database_error)?;
        if inserted > 0 {
            crate::diagnostics::event(
                "internet.search_result",
                serde_json::json!({
                    "job_id": job_id,
                    "provider": provider,
                    "query": query.chars().take(500).collect::<String>(),
                    "title": title.chars().take(300).collect::<String>(),
                    "result_type": result_type,
                    "url": crate::diagnostics::safe_public_url(url),
                }),
            );
        }
        Ok(())
    }

    pub fn record_scholar_candidate_fetch_result(
        &self,
        candidate_id: i64,
        status: &str,
        policy_status: &str,
        policy_reason: Option<&str>,
        fetch_status: &str,
        fetched_url: Option<&str>,
        fetched_excerpt: Option<&str>,
    ) -> io::Result<()> {
        if !matches!(status, "policy_checked" | "fetched" | "skipped" | "failed")
            || !matches!(
                policy_status,
                "allowed" | "disallowed" | "policy_ambiguous" | "unavailable"
            )
            || !matches!(
                fetch_status,
                "not_attempted" | "fetched" | "failed" | "skipped"
            )
        {
            return Err(invalid("invalid source page review state"));
        }
        let changed = self
            .connection
            .execute(
                "UPDATE scholar_search_candidates SET status = ?2, policy_status = ?3,
                policy_reason = ?4, fetch_status = ?5, fetched_url = ?6,
                fetched_excerpt = ?7,
                fetched_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
                params![
                    candidate_id,
                    status,
                    policy_status,
                    policy_reason.map(|value| value.chars().take(500).collect::<String>()),
                    fetch_status,
                    fetched_url,
                    fetched_excerpt.map(|value| value.chars().take(5_000).collect::<String>()),
                ],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("Scholar source candidate was not found"));
        }
        Ok(())
    }

    pub fn record_scholar_candidate_provenance(
        &self,
        candidate_id: i64,
        provenance_json: &str,
    ) -> io::Result<()> {
        if provenance_json.len() > 8_000
            || serde_json::from_str::<serde_json::Value>(provenance_json).is_err()
        {
            return Err(invalid("page provenance must be bounded valid JSON"));
        }
        let changed = self
            .connection
            .execute(
                "UPDATE scholar_search_candidates SET page_provenance_json = ?2
                 WHERE id = ?1 AND status = 'fetched'",
                params![candidate_id, provenance_json],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("fetched Scholar candidate was not found"));
        }
        Ok(())
    }

    pub fn claim_scholar_provenance_review(
        &mut self,
    ) -> io::Result<Option<ScholarProvenanceReview>> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let id: Option<i64> = tx
            .query_row(
                "SELECT id FROM scholar_provenance_reviews
                 WHERE status = 'pending'
                    OR (status = 'failed' AND completed_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-5 minutes'))
                    OR (status = 'processing' AND claimed_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-15 minutes'))
                 ORDER BY id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)?;
        let Some(id) = id else {
            tx.commit().map_err(database_error)?;
            return Ok(None);
        };
        tx.execute(
            "UPDATE scholar_provenance_reviews SET status = 'processing',
                claimed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 AND (status = 'pending'
                OR (status = 'failed' AND completed_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-5 minutes'))
                OR (status = 'processing' AND claimed_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-15 minutes'))) ",
            [id],
        )
        .map_err(database_error)?;
        let review = tx
            .query_row(
                "SELECT r.id, r.finding_id, r.related_finding_id, r.signal,
                    f.source_url, other.source_url, f.source_excerpt, other.source_excerpt,
                    (SELECT c.page_provenance_json FROM scholar_search_candidates c
                     WHERE c.job_id = f.job_id AND c.fetched_url = f.source_url
                     ORDER BY c.id DESC LIMIT 1),
                    (SELECT c.page_provenance_json FROM scholar_search_candidates c
                     WHERE c.job_id = other.job_id AND c.fetched_url = other.source_url
                     ORDER BY c.id DESC LIMIT 1),
                    r.status, r.verdict, r.rationale
                 FROM scholar_provenance_reviews r
                 JOIN scholar_findings f ON f.id = r.finding_id
                 JOIN scholar_findings other ON other.id = r.related_finding_id
                 WHERE r.id = ?1",
                [id],
                |row| {
                    Ok(ScholarProvenanceReview {
                        id: row.get(0)?,
                        finding_id: row.get(1)?,
                        related_finding_id: row.get(2)?,
                        signal: row.get(3)?,
                        source_url: row.get(4)?,
                        related_source_url: row.get(5)?,
                        source_excerpt: row.get(6)?,
                        related_source_excerpt: row.get(7)?,
                        source_metadata_json: row.get(8)?,
                        related_metadata_json: row.get(9)?,
                        status: row.get(10)?,
                        verdict: row.get(11)?,
                        rationale: row.get(12)?,
                    })
                },
            )
            .map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        Ok(Some(review))
    }

    pub fn finish_scholar_provenance_review(
        &mut self,
        review_id: i64,
        verdict: &str,
        rationale: &str,
    ) -> io::Result<()> {
        if !matches!(
            verdict,
            "likely_shared_origin" | "unclear" | "possibly_distinct_reporting"
        ) || rationale.trim().is_empty()
            || rationale.chars().count() > 800
            || rationale.chars().any(char::is_control)
        {
            return Err(invalid("invalid Curmudgeon provenance assessment"));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let finding_id: i64 = tx
            .query_row(
                "SELECT finding_id FROM scholar_provenance_reviews
                 WHERE id = ?1 AND status = 'processing'",
                [review_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let changed = tx
            .execute(
                "UPDATE scholar_provenance_reviews SET status = 'reviewed', verdict = ?2,
                    rationale = ?3, completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ?1 AND status = 'processing'",
                params![review_id, verdict, rationale],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("Curmudgeon provenance review was not processing"));
        }
        tx.execute(
            "UPDATE scholar_source_assessments
             SET reason = substr(reason, 1, 1200) || '; Curmudgeon provenance review ['
                 || ?2 || ']: ' || ?3
             WHERE finding_id = ?1",
            params![finding_id, verdict, rationale],
        )
        .map_err(database_error)?;
        tx.commit().map_err(database_error)
    }

    pub fn fail_scholar_provenance_review(&self, review_id: i64, error: &str) -> io::Result<()> {
        let changed = self
            .connection
            .execute(
                "UPDATE scholar_provenance_reviews SET status = 'failed', error = ?2,
                    completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ?1 AND status = 'processing'",
                params![review_id, error.chars().take(500).collect::<String>()],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("Curmudgeon provenance review was not processing"));
        }
        Ok(())
    }

    pub fn scholar_provenance_reviews(
        &self,
        finding_id: i64,
    ) -> io::Result<Vec<ScholarProvenanceReview>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT r.id, r.finding_id, r.related_finding_id, r.signal,
                f.source_url, other.source_url, f.source_excerpt, other.source_excerpt,
                (SELECT c.page_provenance_json FROM scholar_search_candidates c
                 WHERE c.job_id = f.job_id AND c.fetched_url = f.source_url
                 ORDER BY c.id DESC LIMIT 1),
                (SELECT c.page_provenance_json FROM scholar_search_candidates c
                 WHERE c.job_id = other.job_id AND c.fetched_url = other.source_url
                 ORDER BY c.id DESC LIMIT 1),
                r.status, r.verdict, r.rationale
             FROM scholar_provenance_reviews r
             JOIN scholar_findings f ON f.id = r.finding_id
             JOIN scholar_findings other ON other.id = r.related_finding_id
             WHERE r.finding_id = ?1 OR r.related_finding_id = ?1
             ORDER BY r.id",
            )
            .map_err(database_error)?;
        statement
            .query_map([finding_id], |row| {
                Ok(ScholarProvenanceReview {
                    id: row.get(0)?,
                    finding_id: row.get(1)?,
                    related_finding_id: row.get(2)?,
                    signal: row.get(3)?,
                    source_url: row.get(4)?,
                    related_source_url: row.get(5)?,
                    source_excerpt: row.get(6)?,
                    related_source_excerpt: row.get(7)?,
                    source_metadata_json: row.get(8)?,
                    related_metadata_json: row.get(9)?,
                    status: row.get(10)?,
                    verdict: row.get(11)?,
                    rationale: row.get(12)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn scholar_search_candidates(
        &self,
        job_id: &str,
    ) -> io::Result<Vec<ScholarSearchCandidate>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, provider, query, result_type, title, url, snippet,
                    structured_data_json, status, policy_status, policy_reason,
                    fetch_status, fetched_url, fetched_excerpt, fetched_at,
                    page_provenance_json
                 FROM scholar_search_candidates
             WHERE job_id = ?1 ORDER BY id",
            )
            .map_err(database_error)?;
        statement
            .query_map([job_id], |row| {
                Ok(ScholarSearchCandidate {
                    id: row.get(0)?,
                    provider: row.get(1)?,
                    query: row.get(2)?,
                    result_type: row.get(3)?,
                    title: row.get(4)?,
                    url: row.get(5)?,
                    snippet: row.get(6)?,
                    structured_data_json: row.get(7)?,
                    status: row.get(8)?,
                    policy_status: row.get(9)?,
                    policy_reason: row.get(10)?,
                    fetch_status: row.get(11)?,
                    fetched_url: row.get(12)?,
                    fetched_excerpt: row.get(13)?,
                    fetched_at: row.get(14)?,
                    page_provenance_json: row.get(15)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn record_scholar_extraction_attempt(
        &self,
        candidate_id: i64,
        attempt_index: usize,
        passage: &str,
        status: &str,
        reason: &str,
        model_output: Option<&str>,
    ) -> io::Result<()> {
        if !(1..=3).contains(&attempt_index)
            || passage.trim().is_empty()
            || passage.chars().count() > 2_200
            || !matches!(
                status,
                "accepted"
                    | "no_claim"
                    | "invalid_json"
                    | "model_error"
                    | "low_confidence"
                    | "ungrounded"
                    | "filtered_expansion"
                    | "structured_list"
            )
        {
            return Err(invalid("invalid Scholar extraction attempt"));
        }
        self.connection
            .execute(
                "INSERT INTO scholar_extraction_attempts
                    (candidate_id, attempt_index, passage, status, reason, model_output, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                 ON CONFLICT(candidate_id, attempt_index) DO UPDATE SET
                    passage = excluded.passage, status = excluded.status,
                    reason = excluded.reason, model_output = excluded.model_output,
                    created_at = excluded.created_at",
                params![
                    candidate_id,
                    i64::try_from(attempt_index).unwrap_or(0),
                    passage,
                    status,
                    reason.chars().take(500).collect::<String>(),
                    model_output.map(|output| output.chars().take(4_000).collect::<String>()),
                ],
            )
            .map_err(database_error)?;
        Ok(())
    }

    pub fn scholar_extraction_attempts(
        &self,
        candidate_id: i64,
    ) -> io::Result<Vec<ScholarExtractionAttempt>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT attempt_index, passage, status, reason, model_output
                 FROM scholar_extraction_attempts
                 WHERE candidate_id = ?1 ORDER BY attempt_index",
            )
            .map_err(database_error)?;
        statement
            .query_map([candidate_id], |row| {
                Ok(ScholarExtractionAttempt {
                    attempt_index: usize::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
                    passage: row.get(1)?,
                    status: row.get(2)?,
                    reason: row.get(3)?,
                    model_output: row.get(4)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn record_scholar_search_attempt(
        &self,
        job_id: &str,
        query: &str,
        status: &str,
        result_count: Option<usize>,
        error: Option<&str>,
    ) -> io::Result<()> {
        self.record_scholar_search_attempt_for_provider(
            job_id,
            "wikipedia",
            "primary",
            query,
            status,
            result_count,
            error,
        )
    }

    pub fn record_scholar_search_attempt_for_provider(
        &self,
        job_id: &str,
        provider: &str,
        trigger_reason: &str,
        query: &str,
        status: &str,
        result_count: Option<usize>,
        error: Option<&str>,
    ) -> io::Result<()> {
        if !matches!(status, "results" | "no_result" | "error") {
            return Err(invalid("invalid Scholar search status"));
        }
        self.connection.execute(
            "INSERT INTO scholar_search_attempts
             (job_id, provider, trigger_reason, query, status, result_count, error, created_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7,
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![job_id, provider, trigger_reason, query, status,
                result_count.and_then(|v| i64::try_from(v).ok()), error],
        ).map_err(database_error)?;
        let logged_query = query.chars().take(500).collect::<String>();
        crate::diagnostics::event(
            "internet.search",
            serde_json::json!({
                "job_id": job_id,
                "provider": provider,
                "trigger": trigger_reason,
                "query": logged_query,
                "query_truncated": query.chars().count() > 500,
                "outcome": status,
                "result_count": result_count,
                "error": error,
            }),
        );
        Ok(())
    }

    pub fn scholar_search_attempts(&self, job_id: &str) -> io::Result<Vec<ScholarSearchAttempt>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT provider, trigger_reason, query, status, result_count, error
             FROM scholar_search_attempts WHERE job_id = ?1 ORDER BY id",
            )
            .map_err(database_error)?;
        statement
            .query_map([job_id], |row| {
                Ok(ScholarSearchAttempt {
                    provider: row.get(0)?,
                    trigger_reason: row.get(1)?,
                    query: row.get(2)?,
                    status: row.get(3)?,
                    result_count: row
                        .get::<_, Option<i64>>(4)?
                        .and_then(|count| usize::try_from(count).ok()),
                    error: row.get(5)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }
}
