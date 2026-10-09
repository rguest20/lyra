use super::*;
use sha2::{Digest, Sha256};

pub(super) mod baskets;
mod corroboration;
mod feedback;
mod intake;
pub(crate) mod network;
mod promotion;
use promotion::{automatically_review_source_pair, self_promote_verified_findings};

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

pub(super) fn robots_block_key(path_query: &str) -> String {
    format!("{:x}", Sha256::digest(path_query.as_bytes()))
}

impl LearningStore {
    pub fn last_scholar_provider_search_ms(&self, provider: &str) -> io::Result<Option<i64>> {
        let table = match provider {
            "serpapi" => "serpapi_usage",
            "tavily" => "tavily_usage",
            _ => return Err(invalid("unsupported Scholar search provider")),
        };
        self.connection
            .query_row(
                &format!("SELECT scheduled_at_ms FROM {table} ORDER BY id DESC LIMIT 1"),
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)
    }

    pub fn robots_blocked(&self, host: &str, path_query: &str) -> io::Result<bool> {
        let key = robots_block_key(path_query);
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM scholar_robots_blocks
                 WHERE host = ?1 AND url_key IN (?2, '*')
                   AND expires_at > strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![host, key],
                |row| row.get(0),
            )
            .map_err(database_error)
    }

    pub fn remember_robots_block(
        &self,
        host: &str,
        path_query: &str,
        display_path: &str,
        reason: &str,
        host_wide: bool,
    ) -> io::Result<()> {
        let key = if host_wide {
            "*".to_owned()
        } else {
            robots_block_key(path_query)
        };
        self.connection
            .execute(
                "INSERT INTO scholar_robots_blocks
                    (host, url_key, display_path, reason, checked_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4,
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+30 days'))
                 ON CONFLICT(host, url_key) DO UPDATE SET
                    display_path = excluded.display_path,
                    reason = excluded.reason,
                    checked_at = excluded.checked_at,
                    expires_at = excluded.expires_at",
                params![
                    host,
                    key,
                    if host_wide { "/*" } else { display_path },
                    reason,
                ],
            )
            .map_err(database_error)?;
        Ok(())
    }

    pub fn scholar_robots_blocks(&self, limit: usize) -> io::Result<Vec<ScholarRobotsBlock>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT host, display_path, reason, expires_at
                 FROM scholar_robots_blocks
                 WHERE expires_at > strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 ORDER BY checked_at DESC LIMIT ?1",
            )
            .map_err(database_error)?;
        statement
            .query_map([i64::try_from(limit.min(100)).unwrap_or(100)], |row| {
                Ok(ScholarRobotsBlock {
                    host: row.get(0)?,
                    display_path: row.get(1)?,
                    reason: row.get(2)?,
                    expires_at: row.get(3)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    /// Reserves the single SerpAPI slot only after its persisted cooldown has
    /// elapsed. Reservations are written before issuing network requests.
    pub fn reserve_serpapi_search(
        &mut self,
        minimum_interval_ms: i64,
        job_id: &str,
        query: &str,
    ) -> io::Result<SerpApiReservation> {
        if minimum_interval_ms < 1 || query.trim().is_empty() {
            return Err(invalid("SerpAPI interval must be positive"));
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| io::Error::other(error.to_string()))?
            .as_millis();
        let now = i64::try_from(now_ms).map_err(|_| invalid("search timestamp too large"))?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let previous: Option<i64> = tx
            .query_row(
                "SELECT scheduled_at_ms FROM serpapi_usage ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)?;
        if previous.is_some_and(|last| now < last.saturating_add(minimum_interval_ms)) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "SerpAPI search is cooling down",
            ));
        }
        tx.execute(
            "INSERT INTO serpapi_usage (scheduled_at_ms, outcome, job_id, query)
             VALUES (?1, 'reserved', ?2, ?3)",
            params![now, job_id, query.chars().take(1_000).collect::<String>()],
        )
        .map_err(database_error)?;
        let id = tx.last_insert_rowid();
        tx.commit().map_err(database_error)?;
        Ok(SerpApiReservation {
            id,
            scheduled_at_ms: now,
        })
    }

    pub fn record_serpapi_search_domains(
        &self,
        search_id: i64,
        domains: &[(String, usize)],
    ) -> io::Result<()> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(database_error)?;
        for (domain, result_count) in domains {
            let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
            let domain = domain.strip_prefix("www.").unwrap_or(&domain);
            if domain.is_empty() || *result_count == 0 {
                continue;
            }
            tx.execute(
                "INSERT INTO serpapi_search_domains (search_id, domain, result_count)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(search_id, domain) DO UPDATE SET
                    result_count = excluded.result_count",
                params![
                    search_id,
                    domain,
                    i64::try_from(*result_count).unwrap_or(i64::MAX)
                ],
            )
            .map_err(database_error)?;
        }
        tx.commit().map_err(database_error)
    }

    pub fn serpapi_domain_history(
        &self,
        limit: usize,
    ) -> io::Result<Vec<SerpApiDomainObservation>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT u.scheduled_at_ms, u.query, d.domain, d.result_count
             FROM serpapi_search_domains d
             JOIN serpapi_usage u ON u.id = d.search_id
             ORDER BY u.scheduled_at_ms DESC, d.domain
             LIMIT ?1",
            )
            .map_err(database_error)?;
        statement
            .query_map([i64::try_from(limit.min(1_000)).unwrap_or(1_000)], |row| {
                Ok(SerpApiDomainObservation {
                    searched_at_ms: row.get(0)?,
                    query: row.get(1)?,
                    domain: row.get(2)?,
                    result_count: usize::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn complete_serpapi_search(
        &self,
        request_id: i64,
        response_status: Option<u16>,
        error: Option<&str>,
    ) -> io::Result<()> {
        let completed_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| io::Error::other(error.to_string()))?
            .as_millis();
        let completed_at_ms = i64::try_from(completed_at_ms)
            .map_err(|_| invalid("search completion timestamp too large"))?;
        let changed = self
            .connection
            .execute(
                "UPDATE serpapi_usage SET completed_at_ms = ?2, response_status = ?3,
                    outcome = ?4, error = ?5 WHERE id = ?1 AND outcome = 'reserved'",
                params![
                    request_id,
                    completed_at_ms,
                    response_status,
                    if error.is_some() {
                        "failed"
                    } else {
                        "succeeded"
                    },
                    error.map(|value| value.chars().take(500).collect::<String>()),
                ],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("SerpAPI search reservation was not pending"));
        }
        Ok(())
    }

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
        automatically_review_source_pair(&tx, finding_id)?;
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

    pub fn reserve_tavily_search_with_interval(
        &mut self,
        search_depth: &str,
        minimum_interval_ms: i64,
    ) -> io::Result<TavilyReservation> {
        if minimum_interval_ms < 0 {
            return Err(invalid("Tavily interval must not be negative"));
        }
        let credits = match search_depth {
            "basic" => 1_i64,
            "advanced" => 2_i64,
            _ => return Err(invalid("unsupported Tavily search depth")),
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| io::Error::other(error.to_string()))?
            .as_millis();
        let now = i64::try_from(now_ms).map_err(|_| invalid("search timestamp too large"))?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let previous: Option<i64> = tx
            .query_row(
                "SELECT scheduled_at_ms FROM tavily_usage ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)?;
        if previous.is_some_and(|last| now < last.saturating_add(minimum_interval_ms)) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Tavily search is cooling down",
            ));
        }
        let (daily_calls, month_credits): (i64, i64) = tx
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM tavily_usage
                     WHERE scheduled_at_ms >= ?1),
                    (SELECT COALESCE(SUM(credits), 0) FROM tavily_usage
                     WHERE strftime('%Y-%m', scheduled_at_ms / 1000, 'unixepoch') = strftime('%Y-%m', 'now'))",
                [now.saturating_sub(86_400_000)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(database_error)?;
        if daily_calls >= 25 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Scholar reached the rolling Tavily search-call limit",
            ));
        }
        if month_credits.saturating_add(credits) > 750 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Scholar reached the monthly Tavily credit limit",
            ));
        }
        tx.execute(
            "INSERT INTO tavily_usage
                (scheduled_at_ms, search_depth, credits, outcome)
             VALUES (?1, ?2, ?3, 'reserved')",
            params![now, search_depth, credits],
        )
        .map_err(database_error)?;
        let id = tx.last_insert_rowid();
        tx.commit().map_err(database_error)?;
        Ok(TavilyReservation { id, credits })
    }

    pub fn complete_tavily_search(
        &self,
        request_id: i64,
        response_status: Option<u16>,
        error: Option<&str>,
    ) -> io::Result<()> {
        let completed_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| io::Error::other(error.to_string()))?
            .as_millis();
        let completed_at_ms = i64::try_from(completed_at_ms)
            .map_err(|_| invalid("search completion timestamp too large"))?;
        let changed = self
            .connection
            .execute(
                "UPDATE tavily_usage SET completed_at_ms = ?2, response_status = ?3,
                    outcome = ?4, error = ?5 WHERE id = ?1 AND outcome = 'reserved'",
                params![
                    request_id,
                    completed_at_ms,
                    response_status,
                    if error.is_some() {
                        "failed"
                    } else {
                        "succeeded"
                    },
                    error.map(|value| value.chars().take(500).collect::<String>()),
                ],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("Tavily search reservation was not pending"));
        }
        Ok(())
    }

    /// Atomically reserve a persisted host slot. Unfinished reservations count
    /// against the global and per-host caps until completion or lease expiry.
    pub fn reserve_host_request_with_interval(
        &mut self,
        host: &str,
        request_kind: &str,
        minimum_interval_ms: i64,
    ) -> io::Result<HostRequestReservation> {
        let valid_host = !host.is_empty()
            && host.len() <= 253
            && host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
            && !host.starts_with('.')
            && !host.ends_with('.')
            && !host.contains("..");
        if !valid_host
            || minimum_interval_ms < 10_000
            || !matches!(
                request_kind,
                "wikipedia_search"
                    | "wikipedia_sections"
                    | "wikipedia_page_content"
                    | "external_robots_txt"
                    | "external_llms_txt"
                    | "external_manifest_txt"
                    | "external_page_content"
                    | "serpapi_search"
                    | "tavily_search"
                    | "searxng_search"
            )
        {
            return Err(invalid("unsupported Scholar host request"));
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| io::Error::other(error.to_string()))?
            .as_millis();
        let now = i64::try_from(now_ms).map_err(|_| invalid("request timestamp too large"))?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let retry_until: Option<i64> = tx
            .query_row(
                "SELECT retry_until_ms FROM scholar_host_backoff
                 WHERE host = ?1 AND retry_until_ms > ?2",
                params![host, now],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)?;
        if retry_until.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "host is in persisted Retry-After backoff",
            ));
        }
        network::check_wikipedia_budget(&tx, host, now, network::wikipedia_limits())?;
        let previous: Option<(i64, Option<i64>)> = tx
            .query_row(
                "SELECT scheduled_at_ms, completed_at_ms FROM host_request_log
                 WHERE host = ?1 ORDER BY id DESC LIMIT 1",
                [host],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(database_error)?;
        let minimum_next = previous.map_or(now, |(scheduled, completed)| {
            let spaced_after_reservation = scheduled.saturating_add(minimum_interval_ms);
            let after_completion = completed
                .map(|completed| completed.saturating_add(minimum_interval_ms))
                .unwrap_or_else(|| scheduled.saturating_add(15_000));
            spaced_after_reservation.max(after_completion)
        });
        let scheduled_at_ms = now.max(minimum_next);
        const REQUEST_LEASE_MS: i64 = 30_000;
        let host_in_flight: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM host_request_log
                 WHERE host = ?1 AND outcome = 'reserved' AND lease_until_ms > ?2",
                params![host, now],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let global_in_flight: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM host_request_log
                 WHERE outcome = 'reserved' AND lease_until_ms > ?1",
                [now],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if host_in_flight >= 1 || global_in_flight >= 2 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Scholar HTTP in-flight limit reached",
            ));
        }
        tx.execute(
            "INSERT INTO host_request_log
                (host, request_kind, scheduled_at_ms, outcome, lease_until_ms)
             VALUES (?1, ?2, ?3, 'reserved', ?4)",
            params![
                host,
                request_kind,
                scheduled_at_ms,
                scheduled_at_ms.saturating_add(REQUEST_LEASE_MS)
            ],
        )
        .map_err(database_error)?;
        let id = tx.last_insert_rowid();
        tx.commit().map_err(database_error)?;
        Ok(HostRequestReservation {
            id,
            scheduled_at_ms: u128::try_from(scheduled_at_ms)
                .map_err(|_| invalid("invalid scheduled request timestamp"))?,
        })
    }

    pub fn complete_host_request(
        &mut self,
        request_id: i64,
        response_status: Option<u16>,
        error: Option<&str>,
    ) -> io::Result<()> {
        self.complete_host_request_with_retry_after(request_id, response_status, None, error)
    }

    pub fn complete_host_request_with_retry_after(
        &mut self,
        request_id: i64,
        response_status: Option<u16>,
        retry_after_ms: Option<i64>,
        error: Option<&str>,
    ) -> io::Result<()> {
        let completed_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| io::Error::other(error.to_string()))?
            .as_millis();
        let completed_at_ms = i64::try_from(completed_at_ms)
            .map_err(|_| invalid("request completion timestamp too large"))?;
        let error = error.map(|value| value.chars().take(500).collect::<String>());
        let api_throttled = error.as_deref() == Some(network::MEDIAWIKI_TRANSIENT_ERROR);
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let host: String = tx
            .query_row(
                "SELECT host FROM host_request_log WHERE id = ?1 AND outcome = 'reserved'",
                [request_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("Scholar request reservation was not pending"))?;
        let changed = tx
            .execute(
                "UPDATE host_request_log SET completed_at_ms = ?2, response_status = ?3,
                    outcome = ?4, error = ?5, lease_until_ms = ?2
                 WHERE id = ?1 AND outcome = 'reserved'",
                params![
                    request_id,
                    completed_at_ms,
                    response_status,
                    if error.is_some() || response_status.is_some_and(|code| code >= 300) {
                        "failed"
                    } else {
                        "succeeded"
                    },
                    error.map(|value| value.chars().take(500).collect::<String>()),
                ],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("Scholar request reservation was not pending"));
        }
        if response_status.is_some_and(|code| matches!(code, 429 | 503)) || api_throttled {
            let previous: i64 = tx
                .query_row(
                    "SELECT failures FROM scholar_host_backoff WHERE host=?1",
                    [&host],
                    |row| row.get(0),
                )
                .optional()
                .map_err(database_error)?
                .unwrap_or(0);
            let failures = previous.saturating_add(1).min(7);
            let delay = retry_after_ms
                .unwrap_or(60_000_i64.saturating_mul(1_i64 << (failures - 1)))
                .max(1_000);
            let retry_until = completed_at_ms.saturating_add(delay);
            tx.execute(
                "INSERT INTO scholar_host_backoff (host, retry_until_ms, response_status, updated_at_ms,failures)
                 VALUES (?1, ?2, ?3, ?4,?5)
                 ON CONFLICT(host) DO UPDATE SET retry_until_ms = excluded.retry_until_ms,
                    response_status = excluded.response_status, updated_at_ms = excluded.updated_at_ms,failures=excluded.failures",
                params![host, retry_until, if api_throttled {Some(429)} else {response_status}, completed_at_ms,failures],
            )
            .map_err(database_error)?;
        } else if response_status.is_some_and(|code| code < 400) {
            tx.execute("DELETE FROM scholar_host_backoff WHERE host = ?1", [&host])
                .map_err(database_error)?;
        }
        tx.commit().map_err(database_error)?;
        Ok(())
    }

    pub fn cached_site_policy(
        &self,
        origin: &str,
        request_kind: &str,
    ) -> io::Result<Option<(u16, String, String)>> {
        let row = self
            .connection
            .query_row(
                "SELECT response_status, content_type, body FROM scholar_site_policy_cache
                 WHERE origin = ?1 AND request_kind = ?2
                   AND expires_at_ms > CAST(strftime('%s', 'now') AS INTEGER) * 1000",
                params![origin, request_kind],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(database_error)?;
        Ok(row)
    }

    pub fn cache_site_policy(
        &self,
        origin: &str,
        request_kind: &str,
        response_status: u16,
        content_type: &str,
        body: &str,
        ttl_ms: i64,
    ) -> io::Result<()> {
        if !matches!(
            request_kind,
            "external_robots_txt" | "external_llms_txt" | "external_manifest_txt"
        ) || !matches!(response_status, 200 | 401 | 403 | 404 | 410)
            || ttl_ms <= 0
            || body.len() > 512 * 1024
        {
            return Err(invalid("invalid site-policy cache entry"));
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| io::Error::other(error.to_string()))?
            .as_millis();
        let now = i64::try_from(now_ms).map_err(|_| invalid("policy timestamp too large"))?;
        self.connection
            .execute(
                "INSERT INTO scholar_site_policy_cache
                    (origin, request_kind, response_status, content_type, body, fetched_at_ms, expires_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(origin, request_kind) DO UPDATE SET
                    response_status = excluded.response_status,
                    content_type = excluded.content_type, body = excluded.body,
                    fetched_at_ms = excluded.fetched_at_ms, expires_at_ms = excluded.expires_at_ms",
                params![origin, request_kind, response_status, content_type, body, now, now.saturating_add(ttl_ms)],
            )
            .map_err(database_error)?;
        Ok(())
    }

    pub fn reserve_host_request_wait(
        &mut self,
        host: &str,
        request_kind: &str,
        minimum_interval_ms: i64,
        max_wait: std::time::Duration,
    ) -> io::Result<HostRequestReservation> {
        let deadline = std::time::Instant::now() + max_wait;
        loop {
            match self.reserve_host_request_with_interval(host, request_kind, minimum_interval_ms) {
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        && error.to_string().contains("in-flight")
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                result => return result,
            }
        }
    }

    /// Reserve an attempt before inference so failures and process exits count
    /// against the small independent model-probe budget.
    pub fn begin_model_probe(&mut self, seed: &str, reason: &str) -> io::Result<Option<i64>> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        if scout_queue_block_reason(&tx, scout_min_interval_seconds(), scout_daily_limit())?
            .is_some()
        {
            return Ok(None);
        }
        let recent: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM scout_model_probes
                 WHERE created_at >= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 day')",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let daily_limit = std::env::var("COMMUNITY_SCOUT_MODEL_PROBES_PER_DAY")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value <= 200)
            .unwrap_or(3);
        if daily_limit > 0 && recent >= i64::try_from(daily_limit).unwrap_or(3) {
            return Ok(None);
        }
        let min_interval = scout_min_interval_seconds();
        let cooling_down = min_interval > 0
            && tx
                .query_row(
                    "SELECT EXISTS(
                     SELECT 1 FROM scout_model_probes
                     WHERE created_at >= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?1)
                     UNION ALL
                     SELECT 1 FROM scout_investigations
                     WHERE created_at >= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?1)
                 )",
                    [format!("-{min_interval} seconds")],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
        if cooling_down {
            return Ok(None);
        }
        tx.execute(
            "INSERT INTO scout_model_probes
             (seed, status, selection_reason, created_at)
             VALUES (?1, 'attempting', ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![seed, reason],
        )
        .map_err(database_error)?;
        let id = tx.last_insert_rowid();
        tx.commit().map_err(database_error)?;
        Ok(Some(id))
    }

    pub fn set_model_probe_seed(&self, id: i64, seed: &str) -> io::Result<()> {
        let changed = self
            .connection
            .execute(
                "UPDATE scout_model_probes SET seed = ?2 WHERE id = ?1 AND status = 'attempting'",
                params![id, seed],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("model probe is not attempting"));
        }
        Ok(())
    }

    pub fn complete_model_probe(
        &self,
        id: i64,
        status: &str,
        raw_output: &str,
        topic: Option<&str>,
        question: Option<&str>,
        scholar_job_id: Option<&str>,
    ) -> io::Result<()> {
        if !matches!(status, "queued" | "rejected" | "failed") {
            return Err(invalid("invalid model probe status"));
        }
        let changed = self
            .connection
            .execute(
                "UPDATE scout_model_probes SET status = ?2, raw_output = ?3,
                topic = ?4, question = ?5, scholar_job_id = ?6,
                completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                selection_reason = CASE WHEN ?2='queued' AND selection_reason LIKE 'deferred:%'
                    THEN 'Deferred public lead admitted to research queue' ELSE selection_reason END
             WHERE id = ?1 AND status = 'attempting'",
                params![
                    id,
                    status,
                    raw_output.chars().take(1000).collect::<String>(),
                    topic,
                    question,
                    scholar_job_id
                ],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("model probe is not attempting"));
        }
        Ok(())
    }

    pub fn defer_model_probe(
        &self,
        id: i64,
        raw: &str,
        topic: &str,
        question: &str,
        reason: &str,
    ) -> io::Result<bool> {
        let reason = format!("deferred: {reason}");
        let changed = self.connection.execute("UPDATE scout_model_probes SET raw_output=?2,topic=?3,question=?4,selection_reason=?5
            WHERE id=?1 AND status='attempting' AND (selection_reason<>?5 OR topic IS NULL OR question IS NULL)",
            params![id,raw.chars().take(1000).collect::<String>(),topic,question,reason]).map_err(database_error)?;
        Ok(changed > 0)
    }

    pub fn deferred_model_probe(
        &self,
    ) -> io::Result<Option<(i64, String, String, String, String)>> {
        self.connection.query_row("SELECT id,seed,topic,question,COALESCE(raw_output,'') FROM scout_model_probes
            WHERE status='attempting' AND selection_reason LIKE 'deferred:%' AND topic IS NOT NULL AND question IS NOT NULL
            ORDER BY id LIMIT 1",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)))
            .optional().map_err(database_error)
    }

    pub fn scout_queue_status(&self) -> io::Result<serde_json::Value> {
        let interval = scout_min_interval_seconds();
        let daily_limit = scout_daily_limit();
        let daily: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM scout_investigations
            WHERE created_at >= strftime('%Y-%m-%dT%H:%M:%fZ','now','-1 day')",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let deferred: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM scout_model_probes
            WHERE status='attempting' AND selection_reason LIKE 'deferred:%'",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        Ok(
            serde_json::json!({"min_interval_seconds":interval,"daily_investigation_limit":daily_limit,
            "investigations_last_day":daily,"deferred_model_leads":deferred,"block_reason":scout_queue_block_reason(&self.connection,interval,daily_limit)?}),
        )
    }

    pub fn model_probes(&self) -> io::Result<Vec<ScoutModelProbe>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, status, seed, topic, question, selection_reason, scholar_job_id
             FROM scout_model_probes ORDER BY id DESC LIMIT 30",
            )
            .map_err(database_error)?;
        statement
            .query_map([], |row| {
                Ok(ScoutModelProbe {
                    id: row.get(0)?,
                    status: row.get(1)?,
                    seed: row.get(2)?,
                    topic: row.get(3)?,
                    question: row.get(4)?,
                    selection_reason: row.get(5)?,
                    scholar_job_id: row.get(6)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn recent_model_topics(&self) -> io::Result<Vec<String>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT topic FROM scout_model_probes WHERE status<>'attempting' AND topic IS NOT NULL
             GROUP BY lower(trim(topic)) ORDER BY MAX(id) DESC LIMIT 30",
            )
            .map_err(database_error)?;
        statement
            .query_map([], |row| row.get(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn model_probe_job_id(&self, id: i64) -> io::Result<Option<String>> {
        self.connection
            .query_row(
                "SELECT scholar_job_id FROM scout_investigations
             WHERE trigger_kind = 'model_probe' AND trigger_ref = ?1
             ORDER BY id DESC LIMIT 1",
                [format!("model_probe:{id}")],
                |row| row.get(0),
            )
            .optional()
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

    pub fn scout_questions_for_subject(&self, subject: &str) -> io::Result<Vec<String>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT s.question FROM scout_investigations s
             JOIN semantic_claims c ON s.trigger_ref = 'db:' || c.id
             WHERE lower(trim(c.subject)) = lower(trim(?1))
             ORDER BY s.id DESC LIMIT 10",
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

    /// Conversation questions take precedence over Scout and feedback work.
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
                  AND NOT EXISTS(SELECT 1 FROM scholar_research_feedback f WHERE f.scholar_job_id = j.id)) AS user_requested
                             FROM scholar_jobs j WHERE (status = 'pending' AND (next_attempt_at_ms <= CAST(strftime('%s','now') AS INTEGER)*1000
                                 OR last_error = 'Scholar reached the daily external page-fetch limit'))
               OR (status = 'processing' AND claimed_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-600 seconds'))
             )
             SELECT id, question, status FROM queued WHERE (?1 = 0 OR user_requested)
               AND (?2 IS NULL OR (conversation_id = ?2 AND turn_number = ?3))
             ORDER BY user_requested DESC,
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
                    confidence, status, decision_reason, valid_from, valid_until,
                    source_quality, freshness, source_updated_at
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
                    valid_from: row.get(9)?,
                    valid_until: row.get(10)?,
                    source_quality: row.get(11)?,
                    freshness: row.get(12)?,
                    source_updated_at: row.get(13)?,
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
                    f.freshness, f.source_updated_at, a.topic_class
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

    pub fn review_scholar_source_expertise(
        &mut self,
        finding_id: i64,
        source_kind: &str,
        suitable_for_topic: bool,
        published_at: Option<&str>,
        rationale: &str,
    ) -> io::Result<()> {
        self.review_scholar_source_expertise_with_method(
            finding_id,
            source_kind,
            suitable_for_topic,
            published_at,
            rationale,
            "human",
        )
    }

    pub fn review_scholar_source_expertise_ai(
        &mut self,
        finding_id: i64,
        source_kind: &str,
        suitable_for_topic: bool,
        published_at: Option<&str>,
        rationale: &str,
    ) -> io::Result<()> {
        self.review_scholar_source_expertise_with_method(
            finding_id,
            source_kind,
            suitable_for_topic,
            published_at,
            rationale,
            "automatic",
        )
    }

    fn review_scholar_source_expertise_with_method(
        &mut self,
        finding_id: i64,
        source_kind: &str,
        suitable_for_topic: bool,
        published_at: Option<&str>,
        rationale: &str,
        review_method: &str,
    ) -> io::Result<()> {
        if !matches!(source_kind, "primary" | "secondary") {
            return Err(invalid("source kind must be primary or secondary"));
        }
        if rationale.trim().is_empty()
            || rationale.chars().count() > 500
            || rationale.chars().any(char::is_control)
        {
            return Err(invalid(
                "source review reason must contain 1 to 500 printable characters",
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let finding: (String, String, f64, String) = tx
            .query_row(
                "SELECT evidence_text, status, confidence, source_excerpt
                 FROM scholar_findings WHERE id = ?1",
                [finding_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("no scholar finding with that ID"))?;
        let (evidence_text, finding_status, confidence, source_excerpt) = finding;
        if finding_status != "needs_review" {
            return Err(invalid(
                "source expertise can only be reviewed for a finding awaiting review",
            ));
        }
        if !source_excerpt.contains(&evidence_text) {
            return Err(invalid(
                "finding evidence is not present in its stored source passage",
            ));
        }
        let published_at = if let Some(date) = published_at {
            let normalized: Option<String> = tx
                .query_row("SELECT date(?1)", [date], |row| row.get(0))
                .map_err(database_error)?;
            Some(normalized.ok_or_else(|| invalid("publication date must be a valid ISO date"))?)
        } else {
            None
        };
        let assessment: (String, Option<i64>, bool, String) = tx
            .query_row(
                "SELECT topic_class, direct_support, independence_verified, gate_status
                 FROM scholar_source_assessments WHERE finding_id = ?1",
                [finding_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get::<_, i64>(2)? != 0,
                        row.get(3)?,
                    ))
                },
            )
            .map_err(database_error)?;
        tx.execute(
            "INSERT INTO scholar_source_expertise_reviews
                (finding_id, source_kind, suitable_for_topic, published_at,
                 evidence_text, rationale, reviewed_at, review_method)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6,
                strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?7)",
            params![
                finding_id,
                source_kind,
                i64::from(suitable_for_topic),
                published_at,
                evidence_text,
                rationale,
                review_method,
            ],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_findings SET source_quality = ?2
             WHERE id = ?1",
            params![
                finding_id,
                if suitable_for_topic && source_kind == "primary" {
                    "primary"
                } else if suitable_for_topic {
                    "reference"
                } else {
                    "unknown"
                },
            ],
        )
        .map_err(database_error)?;
        let is_recent = match published_at.as_deref() {
            Some(date) => tx
                .query_row(
                    "SELECT COALESCE(date(?1) > date('now', '-12 months') AND date(?1) <= date('now'), 0)",
                    [date],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(database_error)?,
            None => false,
        };
        let (gate_status, topical_expertise, gate_reason) = if !suitable_for_topic {
            (
                "pending_review",
                "reviewer_found_topic_mismatch",
                "reviewer found the source unsuitable for this claim's topic",
            )
        } else if assessment.1 != Some(1) {
            (
                "unsupported",
                "reviewer_verified_for_claim",
                "source suitability was reviewed, but the stored passage does not directly support this claim",
            )
        } else if confidence < PROMOTION_CONFIDENCE {
            (
                "pending_review",
                "reviewer_verified_for_claim",
                "source suitability was reviewed, but extraction confidence remains below the threshold",
            )
        } else if assessment.0 == "sensitive" {
            (
                "pending_review",
                "reviewer_verified_for_claim",
                "sensitive claims still require explicit finding review",
            )
        } else if assessment.0 == "recent" && !is_recent {
            (
                "unverified_temporal",
                "reviewer_verified_for_claim",
                "recent claim needs a reviewer-confirmed publication date within 12 months",
            )
        } else if source_kind == "primary" {
            (
                "eligible",
                "reviewer_verified_claim_specific_primary",
                "reviewer verified this primary source's expertise for the claim topic and the passage directly supports the claim",
            )
        } else if assessment.2 && assessment.3 == "eligible" {
            (
                "eligible",
                "reviewer_verified_for_claim",
                "source suitability is verified and source independence was already established",
            )
        } else {
            (
                "awaiting_corroboration",
                "reviewer_verified_for_claim",
                "source suitability is verified; a secondary source still needs verified independent corroboration",
            )
        };
        let date_reason = published_at
            .as_deref()
            .map(|date| format!(" Reviewer-confirmed publication date: {date}."))
            .unwrap_or_default();
        tx.execute(
            "UPDATE scholar_source_assessments SET source_kind = ?2,
                topical_expertise = ?3, published_at = ?4, gate_status = ?5,
                reason = ?6 WHERE finding_id = ?1",
            params![
                finding_id,
                source_kind,
                topical_expertise,
                published_at,
                gate_status,
                format!("{gate_reason}.{date_reason} Review reason: {rationale}"),
            ],
        )
        .map_err(database_error)?;
        tx.commit().map_err(database_error)
    }

    pub fn scholar_source_expertise_reviews(
        &self,
        finding_id: i64,
    ) -> io::Result<Vec<ScholarSourceExpertiseReview>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, finding_id, source_kind, suitable_for_topic, published_at,
                evidence_text, rationale, review_method, reviewed_at
             FROM scholar_source_expertise_reviews
             WHERE finding_id = ?1 ORDER BY id",
            )
            .map_err(database_error)?;
        statement
            .query_map([finding_id], |row| {
                Ok(ScholarSourceExpertiseReview {
                    id: row.get(0)?,
                    finding_id: row.get(1)?,
                    source_kind: row.get(2)?,
                    suitable_for_topic: row.get::<_, i64>(3)? != 0,
                    published_at: row.get(4)?,
                    evidence_text: row.get(5)?,
                    rationale: row.get(6)?,
                    review_method: row.get(7)?,
                    reviewed_at: row.get(8)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn review_scholar_source_origin(
        &mut self,
        finding_id: i64,
        underlying_source_id: &str,
        rationale: &str,
    ) -> io::Result<()> {
        let identity = underlying_source_id.trim();
        if identity.is_empty()
            || identity.chars().count() > 200
            || identity.chars().any(char::is_control)
            || identity.contains("<|")
        {
            return Err(invalid(
                "underlying source identity must contain 1 to 200 printable characters",
            ));
        }
        if rationale.trim().is_empty()
            || rationale.chars().count() > 500
            || rationale.chars().any(char::is_control)
        {
            return Err(invalid(
                "source origin review reason must contain 1 to 500 printable characters",
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let finding: (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        ) = tx
            .query_row(
                "SELECT f.status, f.source_excerpt, f.evidence_text, a.source_kind,
                    f.subject, f.relation, f.object,
                    f.valid_from, f.valid_until
                 FROM scholar_findings f
                 JOIN scholar_source_assessments a ON a.finding_id = f.id
                 WHERE f.id = ?1",
                [finding_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("no assessed Scholar finding with that ID"))?;
        let (
            status,
            source_excerpt,
            evidence_text,
            source_kind,
            subject,
            relation,
            object,
            valid_from,
            valid_until,
        ) = finding;
        if status != "needs_review" {
            return Err(invalid(
                "source origin can only be reviewed for a finding awaiting review",
            ));
        }
        if source_kind != "secondary" {
            return Err(invalid(
                "underlying publication identities apply to secondary sources",
            ));
        }
        if !source_excerpt.contains(&evidence_text) {
            return Err(invalid(
                "finding evidence is not present in its stored source passage",
            ));
        }
        let latest_topic_review: Option<bool> = tx
            .query_row(
                "SELECT suitable_for_topic FROM scholar_source_expertise_reviews
                 WHERE finding_id = ?1 ORDER BY id DESC LIMIT 1",
                [finding_id],
                |row| row.get::<_, i64>(0).map(|value| value != 0),
            )
            .optional()
            .map_err(database_error)?;
        if latest_topic_review != Some(true) {
            return Err(invalid(
                "review source suitability for this topic before verifying its origin",
            ));
        }
        let normalized_identity = canonical(identity);
        tx.execute(
            "INSERT INTO scholar_source_origin_reviews
                (finding_id, underlying_source_id, evidence_text, rationale, reviewed_at)
             VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![finding_id, normalized_identity, evidence_text, rationale],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_source_assessments SET gate_status = 'awaiting_corroboration',
                reason = 'underlying publication identity changed; recomputing independent corroboration'
             WHERE gate_status = 'eligible'
               AND reason LIKE '%distinct verified underlying publication identities%'
               AND finding_id IN (
                   SELECT id FROM scholar_findings WHERE lower(trim(subject)) = lower(trim(?1))
                     AND lower(trim(relation)) = lower(trim(?2))
                     AND lower(trim(object)) = lower(trim(?3))
                     AND valid_from IS ?4 AND valid_until IS ?5
               )",
            params![subject, relation, object, valid_from, valid_until],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_source_assessments SET independence_key = ?2,
                independence_verified = 1,
                reason = substr(reason, 1, 1000) || '; reviewer verified underlying publication identity: ' || ?2 || '. Origin review: ' || ?3
             WHERE finding_id = ?1",
            params![finding_id, normalized_identity, rationale],
        )
        .map_err(database_error)?;

        let qualifying: Vec<i64> = {
            let mut statement = tx
                .prepare(
                    "SELECT f.id FROM scholar_findings f
                     JOIN scholar_source_assessments a ON a.finding_id = f.id
                     WHERE lower(trim(f.subject)) = lower(trim(?1))
                       AND lower(trim(f.relation)) = lower(trim(?2))
                       AND lower(trim(f.object)) = lower(trim(?3))
                       AND f.valid_from IS ?4 AND f.valid_until IS ?5
                       AND f.status IN ('needs_review', 'promoted')
                       AND f.confidence >= ?6
                       AND a.source_kind = 'secondary'
                       AND a.direct_support = 1 AND a.independence_verified = 1
                       AND (SELECT suitable_for_topic FROM scholar_source_expertise_reviews er
                            WHERE er.finding_id = f.id ORDER BY er.id DESC LIMIT 1) = 1
                       AND EXISTS (
                         SELECT 1 FROM scholar_findings other
                         JOIN scholar_source_assessments other_a ON other_a.finding_id = other.id
                         WHERE lower(trim(other.subject)) = lower(trim(f.subject))
                           AND lower(trim(other.relation)) = lower(trim(f.relation))
                           AND lower(trim(other.object)) = lower(trim(f.object))
                           AND other.valid_from IS f.valid_from AND other.valid_until IS f.valid_until
                           AND other.id <> f.id AND other.status IN ('needs_review', 'promoted')
                           AND other.confidence >= ?6
                           AND other_a.source_kind = 'secondary'
                           AND other_a.direct_support = 1 AND other_a.independence_verified = 1
                           AND other_a.independence_key <> a.independence_key
                       AND (SELECT suitable_for_topic FROM scholar_source_expertise_reviews other_er
                            WHERE other_er.finding_id = other.id ORDER BY other_er.id DESC LIMIT 1) = 1
                       AND NOT EXISTS (
                         SELECT 1 FROM scholar_provenance_reviews pr
                         WHERE ((pr.finding_id = f.id AND pr.related_finding_id = other.id)
                            OR (pr.finding_id = other.id AND pr.related_finding_id = f.id))
                           AND (pr.status <> 'reviewed' OR pr.verdict <> 'possibly_distinct_reporting')
                       )
                       )
                     ORDER BY f.id",
                )
                .map_err(database_error)?;
            statement
                .query_map(
                    params![
                        subject,
                        relation,
                        object,
                        valid_from,
                        valid_until,
                        automatic_promotion_confidence(),
                    ],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?
        };
        for id in qualifying {
            tx.execute(
                "UPDATE scholar_source_assessments SET gate_status = 'eligible',
                    reason = 'two directly supporting secondary sources have distinct origins; confidence meets the configured automatic acceptance threshold'
                 WHERE finding_id = ?1",
                [id],
            )
            .map_err(database_error)?;
        }
        self_promote_verified_findings(
            &tx,
            &subject,
            &relation,
            &object,
            valid_from.as_deref(),
            valid_until.as_deref(),
        )?;
        tx.commit().map_err(database_error)
    }

    pub fn scholar_source_origin_reviews(
        &self,
        finding_id: i64,
    ) -> io::Result<Vec<ScholarSourceOriginReview>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, finding_id, underlying_source_id, evidence_text, rationale, review_method, reviewed_at
             FROM scholar_source_origin_reviews WHERE finding_id = ?1 ORDER BY id",
            )
            .map_err(database_error)?;
        statement
            .query_map([finding_id], |row| {
                Ok(ScholarSourceOriginReview {
                    id: row.get(0)?,
                    finding_id: row.get(1)?,
                    underlying_source_id: row.get(2)?,
                    evidence_text: row.get(3)?,
                    rationale: row.get(4)?,
                    review_method: row.get(5)?,
                    reviewed_at: row.get(6)?,
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
        self.review_scholar_finding_with_method(finding_id, accept, reason, "human", None)
    }

    pub fn review_scholar_finding_ai(
        &mut self,
        finding_id: i64,
        accept: bool,
        reason: &str,
    ) -> io::Result<()> {
        self.review_scholar_finding_with_method(finding_id, accept, reason, "ai", None)
    }

    #[cfg(test)]
    pub fn reject_scholar_finding_with_feedback(
        &mut self,
        finding_id: i64,
        reason: &str,
        route: &str,
        guidance: &str,
    ) -> io::Result<()> {
        self.review_scholar_finding_with_method(
            finding_id,
            false,
            reason,
            "ai",
            Some((route, guidance, None, None)),
        )
    }

    pub fn reject_scholar_finding_with_source_feedback(
        &mut self,
        finding_id: i64,
        reason: &str,
        route: &str,
        guidance: &str,
        exclusion_reason: Option<&str>,
        suggested_evidence: Option<&str>,
    ) -> io::Result<()> {
        self.review_scholar_finding_with_method(
            finding_id,
            false,
            reason,
            "ai",
            Some((route, guidance, exclusion_reason, suggested_evidence)),
        )
    }

    fn review_scholar_finding_with_method(
        &mut self,
        finding_id: i64,
        accept: bool,
        reason: &str,
        method: &str,
        feedback: Option<(&str, &str, Option<&str>, Option<&str>)>,
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
                "SELECT job_id, source_url, source_excerpt, subject, relation, object, status,
                    valid_from, valid_until
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
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("no scholar finding with that ID"))?;
        let (
            job_id,
            source_url,
            source_excerpt,
            subject,
            relation,
            object,
            status,
            valid_from,
            valid_until,
        ) = finding;
        if status != "needs_review" {
            return Err(invalid("scholar finding is already resolved"));
        }
        if accept {
            if matches!(canonical(&subject).as_str(), "user" | "the user") {
                return Err(invalid(
                    "web evidence cannot establish a personal fact about the user",
                ));
            }
            let saved_support: Option<String> = tx
                .query_row(
                    "SELECT assessment_json FROM scholar_semantic_support WHERE finding_id = ?1",
                    [finding_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(database_error)?;
            if let Some(saved) = saved_support {
                let support: crate::semantic::SemanticSupport =
                    serde_json::from_str(&saved).map_err(invalid_json)?;
                let evidence: String = tx
                    .query_row(
                        "SELECT evidence_text FROM scholar_findings WHERE id = ?1",
                        [finding_id],
                        |row| row.get(0),
                    )
                    .map_err(database_error)?;
                let bound = ScholarFinding {
                    semantic_support: None,
                    subject: subject.clone(),
                    relation: relation.clone(),
                    object: object.clone(),
                    evidence_text: evidence,
                    confidence: 0.0,
                    valid_from: valid_from.clone(),
                    valid_until: valid_until.clone(),
                    source_quality: "unknown".into(),
                    source_updated_at: None,
                };
                if !support.supports(&bound, &source_excerpt) {
                    return Err(invalid(
                        "saved semantic assessment no longer supports this finding",
                    ));
                }
            }
            let direct_support: Option<i64> = tx
                .query_row(
                    "SELECT direct_support FROM scholar_source_assessments WHERE finding_id = ?1",
                    [finding_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(database_error)?
                .flatten();
            if direct_support == Some(0) {
                return Err(invalid(
                    "cannot accept a finding without direct passage support",
                ));
            }
            if direct_support.is_none() {
                // Legacy findings were backfilled as unassessed. Re-check their
                // excerpt before a reviewer can promote them.
                let (source_title, evidence_text): (String, String) = tx
                    .query_row(
                        "SELECT source_title, evidence_text FROM scholar_findings WHERE id = ?1",
                        [finding_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(database_error)?;
                let legacy = ScholarFinding {
                    semantic_support: None,
                    subject: subject.clone(),
                    relation: relation.clone(),
                    object: object.clone(),
                    evidence_text: evidence_text.clone(),
                    confidence: 0.0,
                    valid_from: valid_from.clone(),
                    valid_until: valid_until.clone(),
                    source_quality: "unknown".into(),
                    source_updated_at: None,
                };
                let supported = source_excerpt.contains(&evidence_text)
                    && (evidence_supports_claim(&legacy)
                        || (matches!(
                            relation.as_str(),
                            "has encyclopedia summary" | "has encyclopedia detail"
                        ) && canonical(&subject) == canonical(&source_title)
                            && object == evidence_text));
                if !supported {
                    return Err(invalid("legacy finding lacks direct passage support"));
                }
            }
        }
        let semantic_id = if accept {
            let mut statement = tx
                .prepare(
                    "SELECT id, subject, relation, object, valid_from, valid_until
                 FROM semantic_claims WHERE retracted_by IS NULL",
                )
                .map_err(database_error)?;
            let current = statement
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
            let aliases = super::entities::EntityAliases::load(&tx)?;
            let equivalent = current
                .iter()
                .find(|(_, s, r, o, from, until)| {
                    aliases.same(s, &subject)
                        && canonical(r) == canonical(&relation)
                        && canonical(o) == canonical(&object)
                        && from.as_deref() == valid_from.as_deref()
                        && until.as_deref() == valid_until.as_deref()
                })
                .map(|(id, _, _, _, _, _)| *id);
            if let Some(id) = equivalent {
                Some(id)
            } else {
                let conflicting = current
                    .iter()
                    .find(|(_, s, r, o, from, until)| {
                        aliases.same(s, &subject)
                            && canonical(r) == canonical(&relation)
                            && canonical(o) != canonical(&object)
                            && claim_periods_overlap(
                                valid_from.as_deref(),
                                valid_until.as_deref(),
                                from.as_deref(),
                                until.as_deref(),
                                &today,
                            )
                    })
                    .map(|(id, _, _, _, from, until)| (*id, from.clone(), until.clone()));
                let supersedes_id = if let Some((id, old_from, old_until)) = &conflicting {
                    if old_from.as_deref() == valid_from.as_deref()
                        && old_until.as_deref() == valid_until.as_deref()
                    {
                        tx.execute(
                            "UPDATE semantic_claims SET retracted_by = ?2, status = 'superseded' WHERE id = ?1",
                            params![id, format!("superseded by reviewed scholar finding #{finding_id}: {}", reason.trim())],
                        ).map_err(database_error)?;
                        Some(*id)
                    } else {
                        // A partial time overlap does not invalidate the old claim's
                        // entire period. Keep it active but challenged for review.
                        tx.execute(
                            "UPDATE semantic_claims SET status = 'challenged' WHERE id = ?1",
                            [id],
                        )
                        .map_err(database_error)?;
                        None
                    }
                } else {
                    None
                };
                tx.execute(
                    "INSERT INTO semantic_claims
                        (subject, relation, object, source, supersedes_id, source_observation_id, created_at, valid_from, valid_until)
                     VALUES (?1, ?2, ?3, ?4, ?5, NULL, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?6, ?7)",
                    params![subject, relation, object, source_url, supersedes_id, valid_from, valid_until],
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
        let proposal_id: i64 = tx.query_row(
            "SELECT id FROM role_proposals WHERE source_type = 'scholar_finding' AND source_id = ?1 ORDER BY id DESC LIMIT 1",
            [finding_id.to_string()], |row| row.get(0),
        ).map_err(database_error)?;
        record_policy_decision(
            &tx,
            proposal_id,
            if accept {
                if method == "ai" {
                    "ai_accepted"
                } else {
                    "human_accepted"
                }
            } else {
                if method == "ai" {
                    "ai_rejected"
                } else {
                    "human_rejected"
                }
            },
            reason.trim(),
            semantic_id,
            Some(if accept { "active" } else { "rejected" }),
        )?;
        tx.execute(
            "UPDATE scholar_jobs SET status = ?2, last_error = NULL WHERE id = ?1",
            params![job_id, if accept { "promoted" } else { "no_result" }],
        )
        .map_err(database_error)?;
        if let Some(claim_id) = semantic_id {
            link_claim_evidence(
                &tx,
                claim_id,
                "web",
                Some(&source_url),
                None,
                &source_excerpt,
                "supports",
            )?;
            tx.execute(
                "UPDATE semantic_claims SET status = 'active',
                     checked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                     last_reviewed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                     next_review_at = CASE volatility
                         WHEN 'fast_changing' THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+30 days')
                         WHEN 'slow_changing' THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+180 days')
                         ELSE NULL END
                 WHERE id = ?1 AND retracted_by IS NULL",
                [claim_id],
            ).map_err(database_error)?;
        }
        if let Some((route, guidance, exclusion_reason, suggested_evidence)) = feedback {
            feedback::queue_feedback(
                &tx,
                finding_id,
                route,
                guidance,
                &format!("review:{finding_id}"),
                exclusion_reason,
                suggested_evidence,
            )?;
        } else {
            tx.execute("UPDATE scholar_feedback_sessions SET status = ?2, last_reason = ?3 WHERE root_finding_id = (SELECT root_finding_id FROM scholar_research_feedback WHERE scholar_job_id = ?1)", params![job_id, if accept { "accepted" } else { "exhausted" }, reason]).map_err(database_error)?;
        }
        tx.commit().map_err(database_error)?;
        if feedback.is_some() {
            self.log_scholar_feedback(finding_id)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod scout_admission_tests {
    use super::*;

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
