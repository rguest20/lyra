use super::*;

impl LearningStore {
    pub(super) fn migrate(&mut self) -> io::Result<()> {
        let version: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(database_error)?;
        if version > SCHEMA_VERSION {
            return Err(invalid("learning database was created by a newer version"));
        }
        if version == SCHEMA_VERSION {
            return Ok(());
        }
        if version < 1 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            create_v1(&tx)?;
            tx.pragma_update(None, "user_version", 1)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 2 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE UNIQUE INDEX observations_are_idempotent
                 ON observations(conversation_id, turn_number, raw_text);",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 2)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 3 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch("ALTER TABLE observations ADD COLUMN claimed_at TEXT;")
                .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 3)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 4 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch("ALTER TABLE candidate_claims ADD COLUMN semantic_claim_id INTEGER REFERENCES semantic_claims(id);
                CREATE TABLE semantic_claim_sources (
                    semantic_claim_id INTEGER NOT NULL REFERENCES semantic_claims(id),
                    candidate_id TEXT NOT NULL REFERENCES candidate_claims(id),
                    observation_id TEXT NOT NULL REFERENCES observations(id),
                    PRIMARY KEY (semantic_claim_id, observation_id)
                );")
                .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 4)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 5 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE scholar_jobs (
                    id TEXT PRIMARY KEY,
                    conversation_id TEXT NOT NULL,
                    turn_number INTEGER NOT NULL,
                    question TEXT NOT NULL,
                    status TEXT NOT NULL CHECK (status IN ('pending', 'processing', 'promoted', 'needs_review', 'no_result')),
                    created_at TEXT NOT NULL,
                    claimed_at TEXT,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT,
                    UNIQUE(conversation_id, turn_number)
                );
                CREATE INDEX scholar_jobs_by_status ON scholar_jobs(status, created_at);
                CREATE TABLE scholar_findings (
                    id INTEGER PRIMARY KEY,
                    job_id TEXT NOT NULL REFERENCES scholar_jobs(id),
                    source_title TEXT NOT NULL,
                    source_url TEXT NOT NULL,
                    source_excerpt TEXT NOT NULL,
                    subject TEXT NOT NULL,
                    relation TEXT NOT NULL,
                    object TEXT NOT NULL,
                    evidence_text TEXT NOT NULL,
                    confidence REAL NOT NULL,
                    status TEXT NOT NULL CHECK (status IN ('promoted', 'needs_review', 'rejected')),
                    semantic_claim_id INTEGER REFERENCES semantic_claims(id),
                    decision_reason TEXT
                );"
            ).map_err(database_error)?;
            tx.pragma_update(None, "user_version", 5)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 6 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE assistant_replies (
                    id TEXT PRIMARY KEY,
                    conversation_id TEXT NOT NULL,
                    turn_number INTEGER NOT NULL,
                    reply_text TEXT NOT NULL,
                    status TEXT NOT NULL CHECK (status IN
                        ('pending', 'processing', 'challenged', 'inconclusive', 'no_claims', 'needs_review')),
                    created_at TEXT NOT NULL,
                    claimed_at TEXT,
                    last_error TEXT,
                    UNIQUE(conversation_id, turn_number)
                );
                CREATE INDEX assistant_replies_by_status ON assistant_replies(status, created_at);
                CREATE TABLE assistant_claims (
                    id INTEGER PRIMARY KEY,
                    reply_id TEXT NOT NULL REFERENCES assistant_replies(id),
                    subject TEXT NOT NULL,
                    relation TEXT NOT NULL,
                    object TEXT NOT NULL,
                    evidence_text TEXT NOT NULL,
                    extraction_confidence REAL NOT NULL,
                    confidence_before REAL NOT NULL,
                    confidence_after REAL NOT NULL,
                    verdict TEXT NOT NULL CHECK (verdict IN ('challenged', 'inconclusive')),
                    challenge_type TEXT,
                    reason TEXT NOT NULL,
                    evidence_refs TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );
                CREATE INDEX assistant_claims_by_reply ON assistant_claims(reply_id);"
            ).map_err(database_error)?;
            tx.pragma_update(None, "user_version", 6)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 7 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE scout_investigations (
                    trigger_kind TEXT NOT NULL CHECK (trigger_kind IN ('challenged', 'uncertain', 'sparse')),
                    topic_key TEXT NOT NULL,
                    trigger_ref TEXT NOT NULL,
                    question TEXT NOT NULL,
                    scholar_job_id TEXT NOT NULL REFERENCES scholar_jobs(id),
                    created_at TEXT NOT NULL,
                    PRIMARY KEY (trigger_kind, topic_key)
                );"
            ).map_err(database_error)?;
            tx.pragma_update(None, "user_version", 7)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 8 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE model_claim_approvals (
                    claim_id INTEGER PRIMARY KEY REFERENCES semantic_claims(id),
                    reason TEXT NOT NULL,
                    approved_at TEXT NOT NULL
                );
                CREATE TABLE model_consolidations (
                    id INTEGER PRIMARY KEY,
                    claim_id INTEGER NOT NULL REFERENCES semantic_claims(id),
                    source_checkpoint TEXT NOT NULL,
                    output_checkpoint TEXT NOT NULL,
                    training_loss REAL NOT NULL,
                    created_at TEXT NOT NULL
                );",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 8)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 9 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "ALTER TABLE semantic_claims ADD COLUMN status TEXT NOT NULL DEFAULT 'active'
                    CHECK (status IN ('active', 'uncertain', 'challenged', 'superseded', 'retracted'));
                 ALTER TABLE semantic_claims ADD COLUMN volatility TEXT NOT NULL DEFAULT 'stable'
                    CHECK (volatility IN ('stable', 'slow_changing', 'fast_changing'));
                 ALTER TABLE semantic_claims ADD COLUMN valid_from TEXT;
                 ALTER TABLE semantic_claims ADD COLUMN valid_until TEXT;
                 ALTER TABLE semantic_claims ADD COLUMN observed_at TEXT;
                 ALTER TABLE semantic_claims ADD COLUMN checked_at TEXT;
                 ALTER TABLE semantic_claims ADD COLUMN last_reviewed_at TEXT;
                 ALTER TABLE semantic_claims ADD COLUMN next_review_at TEXT;
                 UPDATE semantic_claims SET status = 'superseded' WHERE retracted_by IS NOT NULL;
                 CREATE INDEX semantic_claims_by_review_due ON semantic_claims(status, next_review_at);",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 9)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 10 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE claim_evidence (
                    id INTEGER PRIMARY KEY,
                    source_kind TEXT NOT NULL CHECK (source_kind IN ('web', 'user_observation', 'manual', 'other')),
                    source_uri TEXT,
                    observation_id TEXT REFERENCES observations(id),
                    excerpt TEXT NOT NULL,
                    published_at TEXT,
                    retrieved_at TEXT,
                    created_at TEXT NOT NULL,
                    UNIQUE(source_kind, source_uri, observation_id, excerpt)
                );
                CREATE TABLE claim_evidence_links (
                    claim_id INTEGER NOT NULL REFERENCES semantic_claims(id),
                    evidence_id INTEGER NOT NULL REFERENCES claim_evidence(id),
                    stance TEXT NOT NULL CHECK (stance IN ('supports', 'contradicts', 'qualifies')),
                    linked_at TEXT NOT NULL,
                    PRIMARY KEY (claim_id, evidence_id, stance)
                );
                CREATE INDEX claim_evidence_links_by_claim ON claim_evidence_links(claim_id);
                INSERT OR IGNORE INTO claim_evidence
                    (source_kind, source_uri, excerpt, retrieved_at, created_at)
                SELECT 'web', f.source_url, f.source_excerpt, j.created_at, j.created_at
                FROM scholar_findings f JOIN scholar_jobs j ON j.id = f.job_id;
                INSERT OR IGNORE INTO claim_evidence_links (claim_id, evidence_id, stance, linked_at)
                SELECT f.semantic_claim_id, e.id,
                    CASE WHEN lower(f.source_excerpt) LIKE '%' || lower(f.evidence_text) || '%'
                         THEN 'supports' ELSE 'qualifies' END,
                    j.created_at
                FROM scholar_findings f JOIN scholar_jobs j ON j.id = f.job_id
                JOIN claim_evidence e
                  ON e.source_kind = 'web' AND e.source_uri = f.source_url
                 AND e.excerpt = f.source_excerpt
                WHERE f.semantic_claim_id IS NOT NULL;
                INSERT OR IGNORE INTO claim_evidence
                    (source_kind, observation_id, excerpt, created_at)
                SELECT 'user_observation', o.id, o.raw_text, o.created_at
                FROM semantic_claim_sources s JOIN observations o ON o.id = s.observation_id;
                INSERT OR IGNORE INTO claim_evidence_links (claim_id, evidence_id, stance, linked_at)
                SELECT s.semantic_claim_id, e.id, 'supports', o.created_at
                FROM semantic_claim_sources s
                JOIN observations o ON o.id = s.observation_id
                JOIN claim_evidence e ON e.source_kind = 'user_observation'
                    AND e.observation_id = o.id AND e.excerpt = o.raw_text;",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 10)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 11 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "ALTER TABLE scout_investigations RENAME TO scout_investigations_old;
                 CREATE TABLE scout_investigations (
                    trigger_kind TEXT NOT NULL CHECK (trigger_kind IN ('challenged', 'uncertain', 'sparse', 'stale')),
                    topic_key TEXT NOT NULL,
                    trigger_ref TEXT NOT NULL,
                    question TEXT NOT NULL,
                    scholar_job_id TEXT NOT NULL REFERENCES scholar_jobs(id),
                    created_at TEXT NOT NULL,
                    PRIMARY KEY (trigger_kind, topic_key)
                 );
                 INSERT INTO scout_investigations SELECT * FROM scout_investigations_old;
                 DROP TABLE scout_investigations_old;",
            ).map_err(database_error)?;
            tx.pragma_update(None, "user_version", 11)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 12 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "ALTER TABLE scholar_findings ADD COLUMN valid_from TEXT;
                 ALTER TABLE scholar_findings ADD COLUMN valid_until TEXT;",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 12)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
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
        Ok(())
    }
}

fn create_v1(tx: &Transaction<'_>) -> io::Result<()> {
    tx.execute_batch(
        "CREATE TABLE observations (
             id TEXT PRIMARY KEY,
             conversation_id TEXT NOT NULL,
             turn_number INTEGER NOT NULL CHECK (turn_number >= 0),
             speaker TEXT NOT NULL CHECK (speaker = 'user'),
             raw_text TEXT NOT NULL,
             created_at TEXT NOT NULL,
             status TEXT NOT NULL CHECK (status IN
                 ('pending', 'processing', 'promoted', 'needs_review', 'rejected')),
             retry_count INTEGER NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
             last_error TEXT
         );

         CREATE TABLE candidate_claims (
             id TEXT PRIMARY KEY,
             observation_id TEXT NOT NULL REFERENCES observations(id),
             subject TEXT NOT NULL,
             relation TEXT NOT NULL,
             object TEXT NOT NULL,
             evidence_text TEXT NOT NULL,
             context_message_ids TEXT NOT NULL DEFAULT '[]',
             resolution TEXT,
             confidence REAL CHECK (confidence IS NULL OR
                 (confidence >= 0.0 AND confidence <= 1.0)),
             status TEXT NOT NULL CHECK (status IN
                 ('proposed', 'promoted', 'needs_review', 'rejected')),
             decision_reason TEXT
         );

         CREATE TABLE semantic_claims (
             id INTEGER PRIMARY KEY,
             subject TEXT NOT NULL,
             relation TEXT NOT NULL,
             object TEXT NOT NULL,
             source TEXT NOT NULL,
             supersedes_id INTEGER REFERENCES semantic_claims(id),
             retracted_by TEXT,
             source_observation_id TEXT REFERENCES observations(id),
             created_at TEXT
         );

         CREATE INDEX observations_by_status ON observations(status, created_at);
         CREATE INDEX candidate_claims_by_observation
             ON candidate_claims(observation_id);
         CREATE INDEX semantic_claims_by_active
             ON semantic_claims(retracted_by, id);",
    )
    .map_err(database_error)
}
