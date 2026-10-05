use super::*;

impl LearningStore {
    pub(super) fn migrate_middle(&mut self, version: i64) -> io::Result<()> {
        if version < 13 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "ALTER TABLE candidate_claims ADD COLUMN valid_from TEXT;
                 ALTER TABLE candidate_claims ADD COLUMN valid_until TEXT;",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 13)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 14 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE role_proposals (
                    id INTEGER PRIMARY KEY,
                    role TEXT NOT NULL,
                    source_type TEXT NOT NULL,
                    source_id TEXT NOT NULL,
                    suggested_action TEXT NOT NULL,
                    target_claim_id INTEGER REFERENCES semantic_claims(id),
                    evidence_refs TEXT NOT NULL,
                    payload TEXT NOT NULL,
                    created_at TEXT NOT NULL
                 );
                 CREATE INDEX role_proposals_by_source ON role_proposals(source_type, source_id);
                 CREATE TABLE policy_decisions (
                    id INTEGER PRIMARY KEY,
                    proposal_id INTEGER NOT NULL REFERENCES role_proposals(id),
                    outcome TEXT NOT NULL,
                    reason TEXT NOT NULL,
                    resulting_claim_id INTEGER REFERENCES semantic_claims(id),
                    resulting_state TEXT,
                    created_at TEXT NOT NULL
                 );
                 CREATE INDEX policy_decisions_by_proposal ON policy_decisions(proposal_id, id);
                 INSERT INTO role_proposals
                    (role, source_type, source_id, suggested_action, target_claim_id,
                     evidence_refs, payload, created_at)
                 SELECT 'curator', 'candidate_claim', id, 'add_claim', semantic_claim_id,
                        '[]', '{}', COALESCE((SELECT created_at FROM observations WHERE observations.id = candidate_claims.observation_id), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                 FROM candidate_claims;
                 INSERT INTO role_proposals
                    (role, source_type, source_id, suggested_action, target_claim_id,
                     evidence_refs, payload, created_at)
                 SELECT 'scholar', 'scholar_finding', CAST(id AS TEXT), 'add_claim', semantic_claim_id,
                        '[]', '{}', strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 FROM scholar_findings;
                 INSERT INTO role_proposals
                    (role, source_type, source_id, suggested_action, target_claim_id,
                     evidence_refs, payload, created_at)
                 SELECT 'curmudgeon', 'assistant_claim', CAST(id AS TEXT),
                        CASE verdict WHEN 'challenged' THEN 'reduce_usable_confidence' ELSE 'retain_uncertain' END,
                        NULL, evidence_refs, '{}', created_at FROM assistant_claims;
                 INSERT INTO role_proposals
                    (role, source_type, source_id, suggested_action, target_claim_id,
                     evidence_refs, payload, created_at)
                 SELECT 'scout', 'scout_investigation', scholar_job_id, 'queue_research', NULL,
                        '[]', '{}', created_at FROM scout_investigations;",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 14)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 15 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "ALTER TABLE scholar_findings ADD COLUMN source_quality TEXT NOT NULL DEFAULT 'unknown';
                 ALTER TABLE scholar_findings ADD COLUMN freshness TEXT NOT NULL DEFAULT 'unknown';
                 ALTER TABLE scholar_findings ADD COLUMN source_updated_at TEXT;",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 15)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 16 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE maintenance_scan_state (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    last_claim_id INTEGER NOT NULL DEFAULT 0,
                    updated_at TEXT NOT NULL
                 );
                 INSERT INTO maintenance_scan_state (id, last_claim_id, updated_at)
                 VALUES (1, 0, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 16)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 17 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE model_checkpoint_lineage (
                    output_checkpoint TEXT PRIMARY KEY,
                    manifest_path TEXT NOT NULL,
                    manifest_json TEXT NOT NULL,
                    created_at TEXT NOT NULL
                 );",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 17)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 18 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE maintenance_passes (
                    id INTEGER PRIMARY KEY,
                    mode TEXT NOT NULL,
                    worker_pid INTEGER NOT NULL,
                    status TEXT NOT NULL CHECK (status IN ('running', 'completed', 'partial_error', 'interrupted')),
                    started_at TEXT NOT NULL,
                    finished_at TEXT,
                    duration_ms INTEGER
                 );
                 CREATE TABLE maintenance_role_runs (
                    id INTEGER PRIMARY KEY,
                    pass_id INTEGER NOT NULL REFERENCES maintenance_passes(id) ON DELETE CASCADE,
                    role TEXT NOT NULL,
                    status TEXT NOT NULL CHECK (status IN ('work', 'idle', 'held', 'error', 'skipped')),
                    processed INTEGER NOT NULL DEFAULT 0,
                    detail TEXT,
                    duration_ms INTEGER,
                    created_at TEXT NOT NULL
                 );
                 CREATE INDEX maintenance_role_runs_by_pass ON maintenance_role_runs(pass_id, id);
                 ALTER TABLE scout_investigations RENAME TO scout_investigations_old;
                 CREATE TABLE scout_investigations (
                    id INTEGER PRIMARY KEY,
                    trigger_kind TEXT NOT NULL CHECK (trigger_kind IN ('challenged', 'uncertain', 'sparse', 'stale')),
                    topic_key TEXT NOT NULL,
                    trigger_ref TEXT NOT NULL,
                    question TEXT NOT NULL,
                    scholar_job_id TEXT NOT NULL UNIQUE REFERENCES scholar_jobs(id),
                    selection_reason TEXT NOT NULL,
                    created_at TEXT NOT NULL
                 );
                 INSERT INTO scout_investigations
                    (trigger_kind, topic_key, trigger_ref, question, scholar_job_id, selection_reason, created_at)
                 SELECT trigger_kind, topic_key, trigger_ref, question, scholar_job_id,
                        'Migrated investigation: ' || trigger_kind, created_at
                 FROM scout_investigations_old;
                 DROP TABLE scout_investigations_old;
                 CREATE INDEX scout_investigations_by_topic
                    ON scout_investigations(trigger_kind, topic_key, id DESC);
                 CREATE INDEX scout_investigations_by_created
                    ON scout_investigations(created_at);",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 18)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 19 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "ALTER TABLE scout_investigations RENAME TO scout_investigations_old;
                 CREATE TABLE scout_investigations (
                    id INTEGER PRIMARY KEY,
                    trigger_kind TEXT NOT NULL CHECK (trigger_kind IN ('challenged', 'uncertain', 'sparse', 'stale', 'expansion')),
                    topic_key TEXT NOT NULL,
                    trigger_ref TEXT NOT NULL,
                    question TEXT NOT NULL,
                    scholar_job_id TEXT NOT NULL UNIQUE REFERENCES scholar_jobs(id),
                    selection_reason TEXT NOT NULL,
                    created_at TEXT NOT NULL
                 );
                 INSERT INTO scout_investigations
                    (id, trigger_kind, topic_key, trigger_ref, question, scholar_job_id, selection_reason, created_at)
                 SELECT id, trigger_kind, topic_key, trigger_ref, question, scholar_job_id, selection_reason, created_at
                 FROM scout_investigations_old;
                 DROP TABLE scout_investigations_old;
                 CREATE INDEX scout_investigations_by_topic
                    ON scout_investigations(trigger_kind, topic_key, id DESC);
                 CREATE INDEX scout_investigations_by_created
                    ON scout_investigations(created_at);",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 19)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 20 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "ALTER TABLE scout_investigations RENAME TO scout_investigations_old;
                 CREATE TABLE scout_investigations (
                    id INTEGER PRIMARY KEY,
                    trigger_kind TEXT NOT NULL CHECK (trigger_kind IN ('challenged', 'uncertain', 'sparse', 'stale', 'expansion', 'model_probe')),
                    topic_key TEXT NOT NULL,
                    trigger_ref TEXT NOT NULL,
                    question TEXT NOT NULL,
                    scholar_job_id TEXT NOT NULL UNIQUE REFERENCES scholar_jobs(id),
                    selection_reason TEXT NOT NULL,
                    created_at TEXT NOT NULL
                 );
                 INSERT INTO scout_investigations
                    (id, trigger_kind, topic_key, trigger_ref, question, scholar_job_id, selection_reason, created_at)
                 SELECT id, trigger_kind, topic_key, trigger_ref, question, scholar_job_id, selection_reason, created_at
                 FROM scout_investigations_old;
                 DROP TABLE scout_investigations_old;
                 CREATE INDEX scout_investigations_by_topic
                    ON scout_investigations(trigger_kind, topic_key, id DESC);
                 CREATE INDEX scout_investigations_by_created
                    ON scout_investigations(created_at);
                 CREATE TABLE IF NOT EXISTS scout_model_probes (
                    id INTEGER PRIMARY KEY,
                    seed TEXT NOT NULL,
                    status TEXT NOT NULL CHECK (status IN ('attempting', 'queued', 'rejected', 'failed')),
                    raw_output TEXT,
                    topic TEXT,
                    question TEXT,
                    selection_reason TEXT NOT NULL,
                    scholar_job_id TEXT REFERENCES scholar_jobs(id),
                    created_at TEXT NOT NULL,
                    completed_at TEXT
                 );
                 CREATE INDEX IF NOT EXISTS scout_model_probes_by_time ON scout_model_probes(created_at);
                 CREATE TABLE IF NOT EXISTS scholar_search_attempts (
                    id INTEGER PRIMARY KEY,
                    job_id TEXT NOT NULL REFERENCES scholar_jobs(id),
                    provider TEXT NOT NULL,
                    trigger_reason TEXT NOT NULL,
                    query TEXT NOT NULL,
                    status TEXT NOT NULL CHECK (status IN ('started', 'results', 'no_result', 'error')),
                    result_count INTEGER,
                    error TEXT,
                    created_at TEXT NOT NULL,
                    completed_at TEXT
                 );
                 CREATE INDEX IF NOT EXISTS scholar_search_attempts_by_job ON scholar_search_attempts(job_id, id);",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 20)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 21 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS scholar_source_assessments (
                    finding_id INTEGER PRIMARY KEY REFERENCES scholar_findings(id),
                    topic_class TEXT NOT NULL,
                    source_kind TEXT NOT NULL,
                    topical_expertise TEXT NOT NULL,
                    direct_support INTEGER CHECK (direct_support IN (0, 1)),
                    independence_key TEXT,
                    independence_verified INTEGER NOT NULL DEFAULT 0 CHECK (independence_verified IN (0, 1)),
                    currency TEXT NOT NULL,
                    published_at TEXT,
                    gate_status TEXT NOT NULL,
                    reason TEXT NOT NULL,
                    created_at TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS scholar_source_assessments_by_key
                    ON scholar_source_assessments(independence_key, gate_status);
                 INSERT OR IGNORE INTO scholar_source_assessments
                    (finding_id, topic_class, source_kind, topical_expertise,
                     direct_support, independence_key, currency, gate_status, reason, created_at)
                 SELECT id, 'unassessed', 'unknown', 'unassessed', NULL, NULL,
                        freshness, 'unassessed',
                        'finding predates per-source assessment; no retrospective verification',
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 FROM scholar_findings;",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 21)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 22 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "ALTER TABLE scholar_source_assessments
                    ADD COLUMN research_relevance TEXT NOT NULL DEFAULT 'unknown'
                    CHECK (research_relevance IN ('direct', 'related', 'tangential', 'unknown'));",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 22)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 23 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS host_request_log (
                    id INTEGER PRIMARY KEY,
                    host TEXT NOT NULL,
                    request_kind TEXT NOT NULL,
                    scheduled_at_ms INTEGER NOT NULL,
                    completed_at_ms INTEGER,
                    response_status INTEGER,
                    outcome TEXT NOT NULL CHECK (outcome IN ('reserved', 'succeeded', 'failed')),
                    error TEXT
                 );
                 CREATE INDEX IF NOT EXISTS host_request_log_by_host_time
                    ON host_request_log(host, scheduled_at_ms DESC);
                 CREATE INDEX IF NOT EXISTS host_request_log_by_kind_time
                    ON host_request_log(request_kind, scheduled_at_ms DESC);",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 23)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 24 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS tavily_usage (
                    id INTEGER PRIMARY KEY,
                    scheduled_at_ms INTEGER NOT NULL,
                    search_depth TEXT NOT NULL CHECK (search_depth IN ('basic', 'advanced')),
                    credits INTEGER NOT NULL CHECK (credits IN (1, 2)),
                    completed_at_ms INTEGER,
                    response_status INTEGER,
                    outcome TEXT NOT NULL CHECK (outcome IN ('reserved', 'succeeded', 'failed')),
                    error TEXT
                 );
                 CREATE INDEX IF NOT EXISTS tavily_usage_by_day
                    ON tavily_usage(scheduled_at_ms DESC);",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 24)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        Ok(())
    }
}
