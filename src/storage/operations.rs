use super::*;

impl LearningStore {
    pub fn start_maintenance_pass(&mut self, mode: &str) -> io::Result<i64> {
        if !matches!(mode, "chat" | "serve" | "once") {
            return Err(invalid("invalid maintenance mode"));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        tx.execute(
            "UPDATE maintenance_passes SET status = 'interrupted',
                finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE status = 'running'
               AND started_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-10 minutes')",
            [],
        )
        .map_err(database_error)?;
        tx.execute(
            "INSERT INTO maintenance_passes (mode, worker_pid, status, started_at)
             VALUES (?1, ?2, 'running', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![mode, i64::from(std::process::id())],
        )
        .map_err(database_error)?;
        let id = tx.last_insert_rowid();
        tx.commit().map_err(database_error)?;
        Ok(id)
    }

    pub fn record_maintenance_role(
        &self,
        pass_id: i64,
        role: &str,
        status: &str,
        processed: usize,
        detail: Option<&str>,
        duration_ms: u64,
    ) -> io::Result<()> {
        if !matches!(role, "curator" | "curmudgeon" | "scout" | "scholar")
            || !matches!(status, "work" | "idle" | "held" | "error" | "skipped")
        {
            return Err(invalid("invalid maintenance role outcome"));
        }
        self.connection
            .execute(
                "INSERT INTO maintenance_role_runs
                 (pass_id, role, status, processed, detail, duration_ms, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6,
                         strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![
                    pass_id,
                    role,
                    status,
                    i64::try_from(processed).map_err(|_| invalid("processed count too large"))?,
                    detail,
                    i64::try_from(duration_ms).map_err(|_| invalid("duration too large"))?,
                ],
            )
            .map_err(database_error)?;
        Ok(())
    }

    pub fn finish_maintenance_pass(&mut self, pass_id: i64) -> io::Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let changed = tx
            .execute(
                "UPDATE maintenance_passes SET
                    status = CASE WHEN EXISTS (
                        SELECT 1 FROM maintenance_role_runs
                        WHERE pass_id = ?1 AND status = 'error'
                    ) THEN 'partial_error' ELSE 'completed' END,
                    finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                    duration_ms = CAST((julianday('now') - julianday(started_at)) * 86400000 AS INTEGER)
                 WHERE id = ?1 AND status = 'running'",
                [pass_id],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("maintenance pass is not running"));
        }
        tx.execute(
            "DELETE FROM maintenance_passes WHERE id IN (
                SELECT id FROM maintenance_passes
                WHERE status != 'running' ORDER BY id DESC LIMIT -1 OFFSET 200
            )",
            [],
        )
        .map_err(database_error)?;
        tx.commit().map_err(database_error)
    }

    pub fn mark_maintenance_worker_interrupted(&self, worker_pid: u32) -> io::Result<()> {
        self.connection
            .execute(
                "UPDATE maintenance_passes SET status = 'interrupted',
                finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE worker_pid = ?1 AND status = 'running'",
                [i64::from(worker_pid)],
            )
            .map_err(database_error)?;
        Ok(())
    }

    pub fn maintenance_recent_passes(&self, limit: usize) -> io::Result<Vec<MaintenancePass>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, mode,
                    CASE WHEN status = 'running' AND
                        started_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-10 minutes')
                    THEN 'interrupted' ELSE status END,
                    started_at, finished_at, duration_ms
                 FROM maintenance_passes ORDER BY id DESC LIMIT ?1",
            )
            .map_err(database_error)?;
        let mut passes = statement
            .query_map(
                [i64::try_from(limit.min(200)).map_err(|_| invalid("limit too large"))?],
                |row| {
                    let duration: Option<i64> = row.get(5)?;
                    Ok(MaintenancePass {
                        id: row.get(0)?,
                        mode: row.get(1)?,
                        status: row.get(2)?,
                        started_at: row.get(3)?,
                        finished_at: row.get(4)?,
                        duration_ms: duration.and_then(|value| u64::try_from(value).ok()),
                        roles: Vec::new(),
                    })
                },
            )
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        let mut roles = self
            .connection
            .prepare(
                "SELECT role, status, processed, detail, duration_ms
                 FROM maintenance_role_runs WHERE pass_id = ?1 ORDER BY id",
            )
            .map_err(database_error)?;
        for pass in &mut passes {
            pass.roles = roles
                .query_map([pass.id], |row| {
                    let processed: i64 = row.get(2)?;
                    let duration: Option<i64> = row.get(4)?;
                    Ok(MaintenanceRoleRun {
                        role: row.get(0)?,
                        status: row.get(1)?,
                        processed: usize::try_from(processed).unwrap_or(0),
                        detail: row.get(3)?,
                        duration_ms: duration.and_then(|value| u64::try_from(value).ok()),
                    })
                })
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?;
        }
        Ok(passes)
    }

    pub fn maintenance_status(&self) -> io::Result<MaintenanceStatus> {
        let queue = |sql: &str| -> io::Result<MaintenanceQueue> {
            let (count, age): (i64, Option<i64>) = self
                .connection
                .query_row(sql, [], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(database_error)?;
            Ok(MaintenanceQueue {
                pending: usize::try_from(count).unwrap_or(usize::MAX),
                oldest_age_seconds: age.and_then(|value| u64::try_from(value).ok()),
            })
        };
        let curator = queue(
            "SELECT COUNT(*), CAST((julianday('now') - julianday(MIN(created_at))) * 86400 AS INTEGER)
             FROM observations WHERE status IN ('pending', 'processing')",
        )?;
        let scout = queue(
            "SELECT COUNT(*), CAST((julianday('now') - julianday(MIN(created_at))) * 86400 AS INTEGER)
             FROM semantic_claims WHERE retracted_by IS NULL AND
               (status = 'challenged' OR (next_review_at IS NOT NULL AND
                next_review_at <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now')))",
        )?;
        let scholar = queue(
            "SELECT COUNT(*), CAST((julianday('now') - julianday(MIN(created_at))) * 86400 AS INTEGER)
             FROM scholar_jobs WHERE status IN ('pending', 'processing')",
        )?;
        let scan_cursor = self
            .connection
            .query_row(
                "SELECT last_claim_id FROM maintenance_scan_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let latest_scout_reason = self
            .connection
            .query_row(
                "SELECT selection_reason FROM scout_investigations ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)?;
        Ok(MaintenanceStatus {
            last_pass: self.maintenance_recent_passes(1)?.into_iter().next(),
            curator,
            scout,
            scholar,
            scan_cursor,
            scout_priority_order: vec![
                "challenged claims",
                "due reviews",
                "sparse map claims",
                "unresolved assistant claims",
            ],
            latest_scout_reason,
        })
    }

    pub fn maintenance_review_counts(&self) -> io::Result<(usize, usize)> {
        let curator: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM observations WHERE status = 'needs_review'",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let scholar: i64 = self
            .connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM scholar_findings WHERE status = 'needs_review') +
                    (SELECT COUNT(*) FROM scholar_jobs WHERE status = 'needs_review')",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        Ok((
            usize::try_from(curator).unwrap_or(usize::MAX),
            usize::try_from(scholar).unwrap_or(usize::MAX),
        ))
    }
}
