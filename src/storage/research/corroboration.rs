use super::*;

pub(super) fn enqueue_corroboration_candidate(
    tx: &Transaction<'_>,
    finding_id: i64,
) -> io::Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO scholar_corroboration_queue
            (finding_id, claim_key, created_at_ms)
         SELECT f.id,
            lower(trim(f.subject)) || char(31) || lower(trim(f.relation)) || char(31) ||
            lower(trim(f.object)) || char(31) || coalesce(f.valid_from, '') || char(31) ||
            coalesce(f.valid_until, ''),
            CAST(strftime('%s', 'now') AS INTEGER) * 1000
         FROM scholar_findings f
         JOIN scholar_source_assessments a ON a.finding_id = f.id
         WHERE f.id = ?1 AND f.status = 'needs_review'
           AND a.gate_status IN ('awaiting_corroboration', 'pending_review')
           AND a.topic_class = 'historical_scientific'
           AND a.source_kind IN ('secondary', 'unknown')
           AND a.direct_support = 1 AND a.research_relevance = 'direct'
           AND f.confidence >= ?2 AND f.source_quality <> 'primary'
           AND lower(trim(f.subject)) NOT IN ('user', 'the user')",
        params![finding_id, automatic_promotion_confidence()],
    )
    .map_err(database_error)?;
    Ok(())
}

fn searxng_ready(tx: &Transaction<'_>, now: i64, interval_ms: i64) -> io::Result<bool> {
    let last: Option<i64> = tx
        .query_row(
            "SELECT CAST(strftime('%s', created_at) AS INTEGER) * 1000
             FROM scholar_search_attempts WHERE provider = 'searxng'
             ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?;
    Ok(last.is_none_or(|last| now >= last.saturating_add(interval_ms)))
}

impl LearningStore {
    pub fn corroboration_queue(&self, limit: usize) -> io::Result<Vec<CorroborationQueueItem>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT q.finding_id, f.subject || ' ' || f.relation || ' ' || f.object,
                        q.status, q.searxng_attempts,
                        q.claimed_provider, q.last_outcome
                 FROM scholar_corroboration_queue q
                 JOIN scholar_findings f ON f.id = q.finding_id
                 ORDER BY q.created_at_ms, q.finding_id LIMIT ?1",
            )
            .map_err(database_error)?;
        statement
            .query_map([sqlite_limit(limit.min(100))?], |row| {
                Ok(CorroborationQueueItem {
                    finding_id: row.get(0)?,
                    claim: row.get(1)?,
                    status: row.get(2)?,
                    searxng_attempts: row.get(3)?,
                    claimed_provider: row.get(4)?,
                    last_outcome: row.get(5)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn claim_corroboration_task(
        &mut self,
        searxng_available: bool,
        interval_ms: i64,
    ) -> io::Result<Option<CorroborationTask>> {
        if interval_ms < 1 {
            return Err(invalid("SearXNG corroboration interval must be positive"));
        }
        let now = super::unix_time_ms_i64("corroboration timestamp too large")?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_jobs SET status = 'no_result', claimed_at = NULL,
                last_error = 'corroboration worker lease expired'
             WHERE id IN (SELECT attempt_job_id FROM scholar_corroboration_queue
                          WHERE status = 'processing' AND claimed_at_ms < ?1)
               AND status = 'processing'",
            [now.saturating_sub(600_000)],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_corroboration_attempts SET outcome = 'interrupted',
                finished_at_ms = ?1
             WHERE outcome = 'processing' AND job_id IN
                (SELECT attempt_job_id FROM scholar_corroboration_queue
                 WHERE status = 'processing' AND claimed_at_ms < ?2)",
            params![now, now.saturating_sub(600_000)],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_corroboration_queue SET status = 'pending',
                claimed_at_ms = NULL, claimed_provider = NULL, attempt_job_id = NULL,
                last_outcome = 'interrupted'
             WHERE status = 'processing' AND claimed_at_ms < ?1",
            [now.saturating_sub(600_000)],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_corroboration_queue SET status = 'done',
                last_outcome = 'finding resolved'
             WHERE status = 'pending' AND finding_id IN
                (SELECT id FROM scholar_findings WHERE status <> 'needs_review')",
            [],
        )
        .map_err(database_error)?;

        if !searxng_available || !searxng_ready(&tx, now, interval_ms)? {
            tx.commit().map_err(database_error)?;
            return Ok(None);
        }
        let selected = tx
            .query_row(
                "SELECT q.finding_id, f.subject, f.relation, f.object,
                        f.valid_from, f.valid_until
                 FROM scholar_corroboration_queue q
                 JOIN scholar_findings f ON f.id = q.finding_id
                 WHERE q.status = 'pending' AND q.searxng_attempts < 3
                   AND q.next_searxng_at_ms <= ?1 AND f.status = 'needs_review'
                 ORDER BY q.searxng_attempts, q.created_at_ms, q.finding_id LIMIT 1",
                [now],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?;
        let Some((finding_id, subject, relation, object, valid_from, valid_until)) = selected
        else {
            tx.commit().map_err(database_error)?;
            return Ok(None);
        };
        let job_id = super::new_scholar_job_id(&tx)?;
        let question = format!("{} {} {}", subject, relation, object);
        tx.execute(
            "INSERT INTO scholar_jobs
                (id, conversation_id, turn_number, question, status, created_at, claimed_at, attempts)
             VALUES (?1, ?2, 0, ?3, 'processing',
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 1)",
            params![job_id, format!("corroboration:{job_id}"), question],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_corroboration_queue SET status = 'processing',
                claimed_at_ms = ?2, claimed_provider = 'searxng', attempt_job_id = ?3,
                searxng_attempts = searxng_attempts + 1, next_searxng_at_ms = ?4
             WHERE finding_id = ?1 AND status = 'pending'",
            params![finding_id, now, job_id, now.saturating_add(interval_ms)],
        )
        .map_err(database_error)?;
        tx.execute(
            "INSERT INTO scholar_corroboration_attempts
                (finding_id, provider, job_id, started_at_ms)
             VALUES (?1, 'searxng', ?2, ?3)",
            params![finding_id, job_id, now],
        )
        .map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        Ok(Some(CorroborationTask {
            finding_id,
            job_id,
            question,
            subject,
            relation,
            object,
            valid_from,
            valid_until,
        }))
    }

    pub fn finish_corroboration_task(
        &self,
        task: &CorroborationTask,
        outcome: &str,
    ) -> io::Result<()> {
        let now = super::unix_time_ms_i64("corroboration timestamp too large")?;
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(database_error)?;
        let status: String = tx
            .query_row(
                "SELECT status FROM scholar_findings WHERE id = ?1",
                [task.finding_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if outcome == "deferred" {
            tx.execute(
                "UPDATE scholar_corroboration_queue SET
                    searxng_attempts = max(0, searxng_attempts - 1),
                    next_searxng_at_ms = 0
                 WHERE finding_id = ?1 AND attempt_job_id = ?2",
                params![task.finding_id, task.job_id],
            )
            .map_err(database_error)?;
        }
        let attempts: i64 = tx
            .query_row(
                "SELECT searxng_attempts FROM scholar_corroboration_queue
                 WHERE finding_id = ?1 AND attempt_job_id = ?2 AND status = 'processing'",
                params![task.finding_id, task.job_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let next_status = if status != "needs_review" {
            "done"
        } else if attempts >= 3 {
            "exhausted"
        } else {
            "pending"
        };
        tx.execute(
            "UPDATE scholar_corroboration_queue SET status = ?2,
                claimed_at_ms = NULL, claimed_provider = NULL, attempt_job_id = NULL,
                last_outcome = ?3 WHERE finding_id = ?1",
            params![task.finding_id, next_status, outcome],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_corroboration_attempts SET outcome = ?2, finished_at_ms = ?3
             WHERE job_id = ?1 AND outcome = 'processing'",
            params![task.job_id, outcome, now],
        )
        .map_err(database_error)?;
        tx.commit().map_err(database_error)
    }

    pub fn corroboration_source_urls(&self, task: &CorroborationTask) -> io::Result<Vec<String>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT source_url FROM scholar_findings
                 WHERE lower(trim(subject)) = lower(trim(?1))
                   AND lower(trim(relation)) = lower(trim(?2))
                   AND lower(trim(object)) = lower(trim(?3))
                   AND valid_from IS ?4 AND valid_until IS ?5
                   AND status IN ('needs_review', 'promoted')",
            )
            .map_err(database_error)?;
        statement
            .query_map(
                params![
                    task.subject,
                    task.relation,
                    task.object,
                    task.valid_from,
                    task.valid_until
                ],
                |row| row.get(0),
            )
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }
}
