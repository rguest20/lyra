//! Scout model-probe budgets and persisted deferred leads.
use super::*;

impl LearningStore {
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

    pub fn deferred_model_probe(&self) -> io::Result<Option<DeferredModelProbe>> {
        self.connection.query_row("SELECT id,seed,topic,question,COALESCE(raw_output,'') FROM scout_model_probes
            WHERE status='attempting' AND selection_reason LIKE 'deferred:%' AND topic IS NOT NULL AND question IS NOT NULL
            ORDER BY id LIMIT 1",[],|row|Ok(DeferredModelProbe {
                id: row.get(0)?,
                seed: row.get(1)?,
                topic: row.get(2)?,
                question: row.get(3)?,
                raw_output: row.get(4)?,
            }))
            .optional().map_err(database_error)
    }

    pub fn scout_queue_status(&self) -> io::Result<serde_json::Value> {
        let interval = scout_min_interval_seconds();
        let daily_limit = scout_daily_limit();
        let daily = scout_investigations_last_day(&self.connection)?;
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
                "SELECT topic FROM scout_model_probes
             WHERE status<>'attempting' AND topic IS NOT NULL
               AND created_at >= strftime('%Y-%m-%dT%H:%M:%fZ','now','-1 day')
             GROUP BY lower(trim(topic)) ORDER BY MAX(id) DESC",
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
}
