//! Durable guided retries shared by Scout, Scholar, and Curmudgeon.

use super::*;

pub(super) fn queue_feedback(
    tx: &Transaction<'_>,
    finding_id: i64,
    route: &str,
    guidance: &str,
    trigger_key: &str,
    exclusion_reason: Option<&str>,
    suggested_evidence: Option<&str>,
) -> io::Result<()> {
    if !matches!(route, "repair" | "new_source") {
        return Err(invalid("feedback route must be repair or new_source"));
    }
    validate_review_reason(guidance)?;
    if exclusion_reason.is_some_and(|reason| !matches!(reason, "irrelevant" | "unreliable"))
        || (exclusion_reason.is_some() && route != "new_source")
    {
        return Err(invalid(
            "source exclusion requires an explicit irrelevant or unreliable verdict and a new_source route",
        ));
    }
    if let Some(evidence) = suggested_evidence {
        let passage: String = tx
            .query_row(
                "SELECT source_excerpt FROM scholar_findings WHERE id = ?1",
                [finding_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if evidence.trim().is_empty()
            || evidence.chars().count() > 1500
            || !passage.contains(evidence)
        {
            return Err(invalid(
                "suggested evidence must be an exact span of 1–1500 characters from the source passage",
            ));
        }
    }
    if tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM scholar_research_feedback WHERE trigger_key = ?1)",
            [trigger_key],
            |row| row.get::<_, bool>(0),
        )
        .map_err(database_error)?
    {
        return Ok(());
    }
    let root: i64 = tx.query_row(
        "SELECT coalesce((SELECT root_finding_id FROM scholar_research_feedback WHERE scholar_job_id = f.job_id), f.id)
         FROM scholar_findings f WHERE f.id = ?1", [finding_id], |row| row.get(0),
    ).map_err(database_error)?;
    tx.execute(
        "INSERT OR IGNORE INTO scholar_feedback_sessions VALUES (?1, 1, 'active', ?2)",
        params![root, guidance],
    )
    .map_err(database_error)?;
    let (attempts, status): (i64, String) = tx
        .query_row(
            "SELECT attempts, status FROM scholar_feedback_sessions WHERE root_finding_id = ?1",
            [root],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(database_error)?;
    if status != "active" {
        return Ok(());
    }
    if attempts >= 3 {
        tx.execute("UPDATE scholar_feedback_sessions SET status = 'exhausted', last_reason = ?2 WHERE root_finding_id = ?1", params![root, guidance]).map_err(database_error)?;
        return Ok(());
    }
    tx.execute("UPDATE scholar_feedback_sessions SET attempts = attempts + 1, last_reason = ?2 WHERE root_finding_id = ?1", params![root, guidance]).map_err(database_error)?;
    tx.execute(
        "INSERT INTO scholar_research_feedback (trigger_key, finding_id, root_finding_id, attempt, route, guidance, status, created_at, exclusion_reason, suggested_evidence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?7, ?8)",
        params![trigger_key, finding_id, root, attempts + 1, route, guidance, exclusion_reason, suggested_evidence],
    ).map_err(database_error)?;
    Ok(())
}

impl LearningStore {
    pub fn log_scholar_feedback(&self, finding_id: i64) -> io::Result<()> {
        let root: i64 = self.connection.query_row(
            "SELECT coalesce((SELECT root_finding_id FROM scholar_research_feedback WHERE scholar_job_id = f.job_id), f.id) FROM scholar_findings f WHERE f.id = ?1",
            [finding_id], |row| row.get(0),
        ).map_err(database_error)?;
        let (attempts, status, guidance): (i64, String, String) = self.connection.query_row(
            "SELECT attempts, status, last_reason FROM scholar_feedback_sessions WHERE root_finding_id = ?1", [root], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).map_err(database_error)?;
        let task: Option<(i64, String, Option<String>)> = self.connection.query_row(
            "SELECT id, route, exclusion_reason FROM scholar_research_feedback WHERE root_finding_id = ?1 ORDER BY attempt DESC LIMIT 1", [root],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional().map_err(database_error)?;
        crate::diagnostics::event(
            "scholar.research_feedback",
            serde_json::json!({
                "finding_id": finding_id, "root_finding_id": root, "attempt": attempts,
                "max_attempts": 3, "status": status, "guidance": guidance,
                "feedback_id": task.as_ref().map(|task| task.0), "route": task.as_ref().map(|task| task.1.as_str()),
                "exclusion_reason": task.as_ref().and_then(|task| task.2.as_deref()),
                "outcome": if status == "exhausted" { "attempt_budget_exhausted" } else { "feedback_queued" },
            }),
        );
        Ok(())
    }

    pub fn claim_scholar_feedback(
        &mut self,
        route: &str,
    ) -> io::Result<Option<ScholarFeedbackTask>> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let task: Option<(i64, Option<String>)> = tx.query_row(
            "SELECT id, scholar_job_id FROM scholar_research_feedback WHERE route = ?1
             AND (status = 'pending' OR (status = 'processing' AND claimed_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-10 minutes')))
             ORDER BY id LIMIT 1", [route], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional().map_err(database_error)?;
        let Some((id, existing_job)) = task else {
            return Ok(None);
        };
        let job_id = match existing_job {
            Some(job) => job,
            None => {
                let job: String = tx
                    .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
                    .map_err(database_error)?;
                tx.execute(
                    "INSERT INTO scholar_jobs (id, conversation_id, turn_number, question, status, created_at)
                     SELECT ?2, 'feedback:' || ?2, 0, j.question, 'pending', strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     FROM scholar_research_feedback r JOIN scholar_findings f ON f.id = r.root_finding_id
                     JOIN scholar_jobs j ON j.id = f.job_id WHERE r.id = ?1", params![id, job],
                ).map_err(database_error)?;
                job
            }
        };
        tx.execute("UPDATE scholar_research_feedback SET status = ?2, scholar_job_id = ?3, claimed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1", params![id, if route == "repair" { "processing" } else { "queued" }, job_id]).map_err(database_error)?;
        if route == "repair" {
            tx.execute("UPDATE scholar_jobs SET status = 'processing', claimed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), attempts = attempts + 1 WHERE id = ?1", [&job_id]).map_err(database_error)?;
        }
        tx.commit().map_err(database_error)?;
        self.scholar_feedback_for_job(&job_id)
    }

    pub fn scholar_feedback_for_job(
        &self,
        job_id: &str,
    ) -> io::Result<Option<ScholarFeedbackTask>> {
        self.connection.query_row(
            "SELECT r.id, r.finding_id, r.root_finding_id, r.attempt, r.guidance, r.scholar_job_id,
                    j.question, f.source_title, f.source_url, f.source_excerpt, f.source_quality, f.source_updated_at,
                    f.subject || ' ' || f.relation || ' ' || f.object, r.suggested_evidence, r.exclusion_reason
             FROM scholar_research_feedback r JOIN scholar_findings f ON f.id = r.finding_id
             JOIN scholar_jobs j ON j.id = r.scholar_job_id WHERE r.scholar_job_id = ?1", [job_id],
            |row| Ok(ScholarFeedbackTask { id: row.get(0)?, finding_id: row.get(1)?, root_finding_id: row.get(2)?, attempt: row.get(3)?, guidance: row.get(4)?, job_id: row.get(5)?, question: row.get(6)?, source_title: row.get(7)?, source_url: row.get(8)?, source_excerpt: row.get(9)?, source_quality: row.get(10)?, source_updated_at: row.get(11)?, rejected_claim: row.get(12)?, suggested_evidence: row.get(13)?, exclusion_reason: row.get(14)? }),
        ).optional().map_err(database_error)
    }

    /// Page exclusions apply only to descendants of the same rejected finding.
    /// Unrelated jobs and other findings, even for the same question, remain eligible.
    pub fn scholar_feedback_excluded_sources(&self, job_id: &str) -> io::Result<Vec<String>> {
        let mut statement = self.connection.prepare(
            "SELECT f.source_url FROM scholar_research_feedback r JOIN scholar_findings f ON f.id = r.finding_id
             WHERE r.exclusion_reason IS NOT NULL AND r.root_finding_id =
                (SELECT root_finding_id FROM scholar_research_feedback WHERE scholar_job_id = ?1)",
        ).map_err(database_error)?;
        statement
            .query_map([job_id], |row| row.get(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn finish_scholar_feedback(
        &mut self,
        task: &ScholarFeedbackTask,
        found: bool,
        note: &str,
    ) -> io::Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let changed = tx.execute("UPDATE scholar_research_feedback SET status = 'completed', result_note = ?2 WHERE id = ?1 AND status <> 'completed'", params![task.id, note]).map_err(database_error)?;
        if changed > 0 && !found {
            let guidance = format!("Previous attempt produced no grounded finding. Seek another suitable source for the original question. Earlier review: {}", task.guidance).chars().take(500).collect::<String>();
            queue_feedback(
                &tx,
                task.finding_id,
                "new_source",
                &guidance,
                &format!("retry:{}", task.id),
                None,
                None,
            )?;
        }
        tx.commit().map_err(database_error)?;
        if !found {
            self.log_scholar_feedback(task.finding_id)?;
        }
        crate::diagnostics::event(
            "scholar.feedback_attempt_finished",
            serde_json::json!({
                "feedback_id": task.id, "root_finding_id": task.root_finding_id,
                "job_id": task.job_id, "attempt": task.attempt, "max_attempts": 3,
                "grounded_finding": found, "note": note,
                "extraction_outcome": if found { "finding_proposed" } else { "extraction_failed" },
                "source_verdict": "unchanged",
            }),
        );
        Ok(())
    }

    pub fn recover_finished_scholar_feedback(&mut self) -> io::Result<()> {
        let job: Option<String> = self.connection.query_row(
            "SELECT r.scholar_job_id FROM scholar_research_feedback r JOIN scholar_jobs j ON j.id = r.scholar_job_id
             WHERE r.status IN ('processing', 'queued') AND j.status NOT IN ('pending', 'processing') ORDER BY r.id LIMIT 1",
            [], |row| row.get(0),
        ).optional().map_err(database_error)?;
        if let Some(job) = job {
            if let Some(task) = self.scholar_feedback_for_job(&job)? {
                let found = !self.scholar_findings(&job)?.is_empty();
                self.finish_scholar_feedback(
                    &task,
                    found,
                    "recovered completed research after interruption",
                )?;
            }
        }
        Ok(())
    }
}
