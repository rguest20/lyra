use super::*;

fn table_has_column(connection: &Connection, table: &str, column: &str) -> io::Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
            rusqlite::params![table, column],
            |row| row.get(0),
        )
        .map_err(database_error)
}

fn table_definition(connection: &Connection, table: &str) -> io::Result<Option<String>> {
    connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)
}

impl LearningStore {
    pub(super) fn migrate_recent(&mut self, version: i64) -> io::Result<()> {
        if version < 25 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS serpapi_usage (
                    id INTEGER PRIMARY KEY,
                    scheduled_at_ms INTEGER NOT NULL,
                    completed_at_ms INTEGER,
                    response_status INTEGER,
                    outcome TEXT NOT NULL CHECK (outcome IN ('reserved', 'succeeded', 'failed')),
                    error TEXT
                 );
                 CREATE INDEX IF NOT EXISTS serpapi_usage_by_time
                    ON serpapi_usage(scheduled_at_ms DESC);
                 CREATE TABLE IF NOT EXISTS scholar_search_candidates (
                    id INTEGER PRIMARY KEY,
                    job_id TEXT NOT NULL REFERENCES scholar_jobs(id),
                    provider TEXT NOT NULL,
                    title TEXT NOT NULL,
                    url TEXT NOT NULL,
                    snippet TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'discovered'
                        CHECK (status IN ('discovered', 'policy_checked', 'fetched', 'skipped', 'failed')),
                    created_at TEXT NOT NULL,
                    UNIQUE(job_id, provider, url)
                 );
                 CREATE INDEX IF NOT EXISTS scholar_search_candidates_by_status
                    ON scholar_search_candidates(status, id);
                 PRAGMA user_version = 25;",
            )
            .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 26 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            let has_job_id: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('serpapi_usage') WHERE name = 'job_id')",
                    [],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            let has_query: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('serpapi_usage') WHERE name = 'query')",
                    [],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            if !has_job_id {
                tx.execute_batch(
                    "ALTER TABLE serpapi_usage ADD COLUMN job_id TEXT REFERENCES scholar_jobs(id);",
                )
                .map_err(database_error)?;
            }
            if !has_query {
                tx.execute_batch(
                    "ALTER TABLE serpapi_usage ADD COLUMN query TEXT NOT NULL DEFAULT '';",
                )
                .map_err(database_error)?;
            }
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS serpapi_search_domains (
                    id INTEGER PRIMARY KEY,
                    search_id INTEGER NOT NULL REFERENCES serpapi_usage(id) ON DELETE CASCADE,
                    domain TEXT NOT NULL,
                    result_count INTEGER NOT NULL CHECK (result_count > 0),
                    UNIQUE(search_id, domain)
                 );
                 CREATE INDEX IF NOT EXISTS serpapi_domains_by_domain
                    ON serpapi_search_domains(domain, search_id DESC);
                 PRAGMA user_version = 26;",
            )
            .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 27 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            let has_query: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('scholar_search_candidates') WHERE name = 'query')",
                    [],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            if !has_query {
                tx.execute_batch("ALTER TABLE scholar_search_candidates ADD COLUMN query TEXT NOT NULL DEFAULT ''; ALTER TABLE scholar_search_candidates ADD COLUMN result_type TEXT NOT NULL DEFAULT 'search_result'; ALTER TABLE scholar_search_candidates ADD COLUMN structured_data_json TEXT NOT NULL DEFAULT '{}';")
                    .map_err(database_error)?;
            } else {
                let has_result_type: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('scholar_search_candidates') WHERE name = 'result_type')",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(database_error)?;
                if !has_result_type {
                    tx.execute_batch("ALTER TABLE scholar_search_candidates ADD COLUMN result_type TEXT NOT NULL DEFAULT 'search_result';")
                        .map_err(database_error)?;
                }
                let has_structured_data: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('scholar_search_candidates') WHERE name = 'structured_data_json')",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(database_error)?;
                if !has_structured_data {
                    tx.execute_batch("ALTER TABLE scholar_search_candidates ADD COLUMN structured_data_json TEXT NOT NULL DEFAULT '{}';")
                        .map_err(database_error)?;
                }
            }
            tx.pragma_update(None, "user_version", 27)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 28 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            for (column, definition) in [
                ("policy_status", "TEXT NOT NULL DEFAULT 'not_checked'"),
                ("policy_reason", "TEXT"),
                ("fetch_status", "TEXT NOT NULL DEFAULT 'not_attempted'"),
                ("fetched_url", "TEXT"),
                ("fetched_excerpt", "TEXT"),
                ("fetched_at", "TEXT"),
            ] {
                let exists: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('scholar_search_candidates') WHERE name = ?1)",
                        [column],
                        |row| row.get(0),
                    )
                    .map_err(database_error)?;
                if !exists {
                    tx.execute_batch(&format!(
                        "ALTER TABLE scholar_search_candidates ADD COLUMN {column} {definition};"
                    ))
                    .map_err(database_error)?;
                }
            }
            tx.pragma_update(None, "user_version", 28)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 29 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS scholar_robots_blocks (
                    host TEXT NOT NULL,
                    url_key TEXT NOT NULL,
                    display_path TEXT NOT NULL,
                    reason TEXT NOT NULL,
                    checked_at TEXT NOT NULL,
                    expires_at TEXT NOT NULL,
                    PRIMARY KEY(host, url_key)
                 );
                 CREATE INDEX IF NOT EXISTS scholar_robots_blocks_by_expiry
                    ON scholar_robots_blocks(expires_at);",
            )
            .map_err(database_error)?;
            let previous_denials = {
                let mut statement = tx
                    .prepare(
                        "SELECT url, policy_reason FROM scholar_search_candidates
                         WHERE policy_status = 'disallowed'
                           AND (policy_reason LIKE 'robots.txt disallows%'
                             OR policy_reason LIKE 'robots.txt returned HTTP 401%'
                             OR policy_reason LIKE 'robots.txt returned HTTP 403%')",
                    )
                    .map_err(database_error)?;
                statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(database_error)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(database_error)?
            };
            for (source_url, reason) in previous_denials {
                let Ok(url) = url::Url::parse(&source_url) else {
                    continue;
                };
                let Some(host) = url.host_str() else {
                    continue;
                };
                let host_wide = reason.starts_with("robots.txt returned HTTP 401")
                    || reason.starts_with("robots.txt returned HTTP 403");
                let mut path_query = url.path().to_owned();
                if let Some(query) = url.query() {
                    path_query.push('?');
                    path_query.push_str(query);
                }
                let url_key = if host_wide {
                    "*".to_owned()
                } else {
                    super::research::robots_block_key(&path_query)
                };
                tx.execute(
                    "INSERT OR IGNORE INTO scholar_robots_blocks
                        (host, url_key, display_path, reason, checked_at, expires_at)
                     VALUES (?1, ?2, ?3, ?4,
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+30 days'))",
                    params![
                        host.to_ascii_lowercase(),
                        url_key,
                        if host_wide { "/*" } else { url.path() },
                        reason,
                    ],
                )
                .map_err(database_error)?;
            }
            tx.pragma_update(None, "user_version", 29)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 30 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS scholar_extraction_attempts (
                    id INTEGER PRIMARY KEY,
                    candidate_id INTEGER NOT NULL REFERENCES scholar_search_candidates(id),
                    attempt_index INTEGER NOT NULL CHECK(attempt_index BETWEEN 1 AND 3),
                    passage TEXT NOT NULL,
                    status TEXT NOT NULL CHECK(status IN
                        ('accepted', 'no_claim', 'invalid_json', 'model_error',
                         'low_confidence', 'ungrounded', 'filtered_expansion',
                         'structured_list')),
                    reason TEXT NOT NULL,
                    model_output TEXT,
                    created_at TEXT NOT NULL,
                    UNIQUE(candidate_id, attempt_index)
                 );
                 CREATE INDEX IF NOT EXISTS scholar_extraction_attempts_by_candidate
                    ON scholar_extraction_attempts(candidate_id, attempt_index);",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 30)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 31 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            let table_sql: String = tx
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'table'
                     AND name = 'scholar_extraction_attempts'",
                    [],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            if !table_sql.contains("BETWEEN 1 AND 3") || !table_sql.contains("structured_list") {
                tx.execute_batch(
                    "CREATE TABLE scholar_extraction_attempts_new (
                        id INTEGER PRIMARY KEY,
                        candidate_id INTEGER NOT NULL REFERENCES scholar_search_candidates(id),
                        attempt_index INTEGER NOT NULL CHECK(attempt_index BETWEEN 1 AND 3),
                        passage TEXT NOT NULL,
                        status TEXT NOT NULL CHECK(status IN
                            ('accepted', 'no_claim', 'invalid_json', 'model_error',
                             'low_confidence', 'ungrounded', 'filtered_expansion',
                             'structured_list')),
                        reason TEXT NOT NULL,
                        model_output TEXT,
                        created_at TEXT NOT NULL,
                        UNIQUE(candidate_id, attempt_index)
                     );
                     INSERT INTO scholar_extraction_attempts_new
                        (id, candidate_id, attempt_index, passage, status, reason,
                         model_output, created_at)
                     SELECT id, candidate_id, attempt_index, passage, status, reason,
                            model_output, created_at
                     FROM scholar_extraction_attempts;
                     DROP TABLE scholar_extraction_attempts;
                     ALTER TABLE scholar_extraction_attempts_new
                        RENAME TO scholar_extraction_attempts;
                     CREATE INDEX scholar_extraction_attempts_by_candidate
                        ON scholar_extraction_attempts(candidate_id, attempt_index);",
                )
                .map_err(database_error)?;
            }
            tx.pragma_update(None, "user_version", 31)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 32 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('scholar_search_candidates') WHERE name = 'page_provenance_json')",
                    [],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            if !exists {
                tx.execute_batch(
                    "ALTER TABLE scholar_search_candidates ADD COLUMN page_provenance_json TEXT;",
                )
                .map_err(database_error)?;
            }
            tx.pragma_update(None, "user_version", 32)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 33 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS scholar_provenance_reviews (
                    id INTEGER PRIMARY KEY,
                    finding_id INTEGER NOT NULL REFERENCES scholar_findings(id),
                    related_finding_id INTEGER NOT NULL REFERENCES scholar_findings(id),
                    signal TEXT NOT NULL,
                    status TEXT NOT NULL CHECK(status IN ('pending', 'processing', 'reviewed', 'failed')),
                    verdict TEXT CHECK(verdict IN ('likely_shared_origin', 'unclear', 'possibly_distinct_reporting')),
                    rationale TEXT,
                    error TEXT,
                    created_at TEXT NOT NULL,
                    claimed_at TEXT,
                    completed_at TEXT,
                    UNIQUE(finding_id, related_finding_id)
                 );
                 CREATE INDEX IF NOT EXISTS scholar_provenance_reviews_by_status
                    ON scholar_provenance_reviews(status, id);",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 33)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 34 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS scholar_source_expertise_reviews (
                    id INTEGER PRIMARY KEY,
                    finding_id INTEGER NOT NULL REFERENCES scholar_findings(id),
                    source_kind TEXT NOT NULL CHECK(source_kind IN ('primary', 'secondary')),
                    suitable_for_topic INTEGER NOT NULL CHECK(suitable_for_topic IN (0, 1)),
                    published_at TEXT,
                    evidence_text TEXT NOT NULL,
                    rationale TEXT NOT NULL,
                    reviewed_at TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS scholar_source_expertise_reviews_by_finding
                    ON scholar_source_expertise_reviews(finding_id, id DESC);",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 34)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 35 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS scholar_source_origin_reviews (
                    id INTEGER PRIMARY KEY,
                    finding_id INTEGER NOT NULL REFERENCES scholar_findings(id),
                    underlying_source_id TEXT NOT NULL,
                    evidence_text TEXT NOT NULL,
                    rationale TEXT NOT NULL,
                    reviewed_at TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS scholar_source_origin_reviews_by_finding
                    ON scholar_source_origin_reviews(finding_id, id DESC);",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 35)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 36 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            for table in [
                "scholar_source_expertise_reviews",
                "scholar_source_origin_reviews",
            ] {
                let has_review_method: bool = tx
                    .prepare(&format!("PRAGMA table_info({table})"))
                    .map_err(database_error)?
                    .query_map([], |row| row.get::<_, String>(1))
                    .map_err(database_error)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(database_error)?
                    .iter()
                    .any(|column| column == "review_method");
                if !has_review_method {
                    tx.execute_batch(&format!(
                        "ALTER TABLE {table}
                         ADD COLUMN review_method TEXT NOT NULL DEFAULT 'human'
                         CHECK (review_method IN ('human', 'automatic'));"
                    ))
                    .map_err(database_error)?;
                }
            }
            tx.pragma_update(None, "user_version", 36)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 37 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS scholar_corroboration_queue (
                    finding_id INTEGER PRIMARY KEY REFERENCES scholar_findings(id),
                    claim_key TEXT NOT NULL UNIQUE,
                    status TEXT NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending', 'processing', 'done', 'exhausted')),
                    serp_attempts INTEGER NOT NULL DEFAULT 0,
                    tavily_attempts INTEGER NOT NULL DEFAULT 0,
                    next_serp_at_ms INTEGER NOT NULL DEFAULT 0,
                    next_tavily_at_ms INTEGER NOT NULL DEFAULT 0,
                    claimed_at_ms INTEGER,
                    claimed_provider TEXT CHECK (claimed_provider IN ('serpapi', 'tavily')),
                    attempt_job_id TEXT REFERENCES scholar_jobs(id),
                    created_at_ms INTEGER NOT NULL,
                    last_outcome TEXT
                 );
                 CREATE INDEX IF NOT EXISTS scholar_corroboration_queue_by_status
                    ON scholar_corroboration_queue(status, created_at_ms);
                 CREATE TABLE IF NOT EXISTS scholar_corroboration_attempts (
                    id INTEGER PRIMARY KEY,
                    finding_id INTEGER NOT NULL REFERENCES scholar_corroboration_queue(finding_id),
                    provider TEXT NOT NULL CHECK (provider IN ('serpapi', 'tavily')),
                    job_id TEXT NOT NULL UNIQUE REFERENCES scholar_jobs(id),
                    started_at_ms INTEGER NOT NULL,
                    finished_at_ms INTEGER,
                    outcome TEXT NOT NULL DEFAULT 'processing'
                 );
                 CREATE INDEX IF NOT EXISTS scholar_corroboration_attempts_by_finding
                    ON scholar_corroboration_attempts(finding_id, id DESC);",
            )
            .map_err(database_error)?;
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
                 WHERE f.status = 'needs_review'
                   AND a.gate_status IN ('awaiting_corroboration', 'pending_review')
                   AND a.topic_class = 'historical_scientific'
                   AND a.source_kind IN ('secondary', 'unknown')
                   AND a.direct_support = 1 AND a.research_relevance = 'direct'
                   AND f.confidence >= ?1
                   AND f.source_quality <> 'primary'
                   AND lower(trim(f.subject)) NOT IN ('user', 'the user')
                 ORDER BY f.id",
                [automatic_promotion_confidence()],
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 37)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 38 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            let host_request_columns = tx
                .prepare("PRAGMA table_info(host_request_log)")
                .map_err(database_error)?
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?;
            if !host_request_columns
                .iter()
                .any(|column| column == "lease_until_ms")
            {
                tx.execute_batch("ALTER TABLE host_request_log ADD COLUMN lease_until_ms INTEGER;")
                    .map_err(database_error)?;
            }
            tx.execute_batch(
                "UPDATE host_request_log SET lease_until_ms =
                    CASE WHEN completed_at_ms IS NULL
                         THEN scheduled_at_ms + 30000
                         ELSE completed_at_ms END
                    WHERE lease_until_ms IS NULL;
                 CREATE INDEX IF NOT EXISTS host_request_log_by_lease
                    ON host_request_log(outcome, lease_until_ms);
                 CREATE TABLE IF NOT EXISTS scholar_host_backoff (
                    host TEXT PRIMARY KEY,
                    retry_until_ms INTEGER NOT NULL,
                    response_status INTEGER NOT NULL CHECK (response_status IN (429, 503)),
                    updated_at_ms INTEGER NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS scholar_site_policy_cache (
                    origin TEXT NOT NULL,
                    request_kind TEXT NOT NULL CHECK (request_kind IN (
                        'external_robots_txt', 'external_llms_txt', 'external_manifest_txt'
                    )),
                    response_status INTEGER NOT NULL,
                    content_type TEXT NOT NULL,
                    body TEXT NOT NULL,
                    fetched_at_ms INTEGER NOT NULL,
                    expires_at_ms INTEGER NOT NULL,
                    PRIMARY KEY (origin, request_kind)
                 );
                 CREATE INDEX IF NOT EXISTS scholar_site_policy_cache_by_expiry
                    ON scholar_site_policy_cache(expires_at_ms);
                 PRAGMA user_version = 38;",
            )
            .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 39 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS scholar_ai_review_retries (
                    finding_id INTEGER PRIMARY KEY REFERENCES scholar_findings(id),
                    attempts INTEGER NOT NULL,
                    retry_after TEXT NOT NULL,
                    last_error TEXT NOT NULL
                 );
                 PRAGMA user_version = 39;",
            )
            .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 40 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS scholar_feedback_sessions (
                    root_finding_id INTEGER PRIMARY KEY REFERENCES scholar_findings(id),
                    attempts INTEGER NOT NULL CHECK(attempts BETWEEN 1 AND 3),
                    status TEXT NOT NULL CHECK(status IN ('active', 'exhausted', 'accepted')),
                    last_reason TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS scholar_research_feedback (
                    id INTEGER PRIMARY KEY,
                    trigger_key TEXT NOT NULL UNIQUE,
                    finding_id INTEGER NOT NULL REFERENCES scholar_findings(id),
                    root_finding_id INTEGER NOT NULL REFERENCES scholar_feedback_sessions(root_finding_id),
                    attempt INTEGER NOT NULL CHECK(attempt BETWEEN 2 AND 3),
                    route TEXT NOT NULL CHECK(route IN ('repair', 'new_source')),
                    guidance TEXT NOT NULL,
                    status TEXT NOT NULL CHECK(status IN ('pending','processing','queued','completed')),
                    scholar_job_id TEXT UNIQUE REFERENCES scholar_jobs(id),
                    claimed_at TEXT,
                    created_at TEXT NOT NULL,
                    result_note TEXT,
                    UNIQUE(root_finding_id, attempt)
                 );
                 PRAGMA user_version = 40;",
            ).map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 41 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            if !tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('scholar_research_feedback') WHERE name = 'exclusion_reason')", [], |row| row.get::<_, bool>(0)).map_err(database_error)? {
                tx.execute_batch("ALTER TABLE scholar_research_feedback ADD COLUMN exclusion_reason TEXT CHECK(exclusion_reason IN ('irrelevant','unreliable'));
                                  ALTER TABLE scholar_research_feedback ADD COLUMN suggested_evidence TEXT;").map_err(database_error)?;
            }
            tx.execute_batch("PRAGMA user_version = 41;")
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 42 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS scholar_semantic_support (
                finding_id INTEGER PRIMARY KEY REFERENCES scholar_findings(id),
                assessment_json TEXT NOT NULL,
                assessed_at TEXT NOT NULL
            ); PRAGMA user_version = 42;",
            )
            .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 43 {
            self.connection
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS scholar_lookup_plans (
                   job_id TEXT PRIMARY KEY REFERENCES scholar_jobs(id),
                   queries_json TEXT NOT NULL
                 ); PRAGMA user_version = 43;",
                )
                .map_err(database_error)?;
        }
        if version < 44 {
            self.connection
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS entity_alias_links (
                    left_name TEXT NOT NULL, right_name TEXT NOT NULL,
                    source_url TEXT NOT NULL, evidence_text TEXT NOT NULL,
                    source_finding_id INTEGER REFERENCES scholar_findings(id),
                    assessment_json TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                    PRIMARY KEY (left_name, right_name, source_url)
                );
                CREATE VIEW IF NOT EXISTS active_entity_aliases AS
                    SELECT left_name, right_name FROM entity_alias_links l
                    WHERE l.source_finding_id IS NULL OR EXISTS (
                        SELECT 1 FROM scholar_findings f WHERE f.id = l.source_finding_id
                        AND (f.status = 'needs_review' OR (f.status = 'promoted' AND EXISTS (
                            SELECT 1 FROM semantic_claims c WHERE c.id = f.semantic_claim_id
                            AND c.retracted_by IS NULL AND c.status IN ('active','uncertain')))));
                CREATE VIEW IF NOT EXISTS entity_alias_candidates AS
                    SELECT left_name,right_name FROM active_entity_aliases
                    UNION SELECT right_name,left_name FROM active_entity_aliases;
                CREATE VIEW IF NOT EXISTS entity_equivalences AS
                    SELECT a.left_name,a.right_name FROM entity_alias_candidates a
                    WHERE (SELECT COUNT(DISTINCT b.right_name) FROM entity_alias_candidates b
                           WHERE b.left_name=a.left_name) = 1
                      AND (SELECT COUNT(DISTINCT b.right_name) FROM entity_alias_candidates b
                           WHERE b.left_name=a.right_name) = 1;
                PRAGMA user_version = 44;",
                )
                .map_err(database_error)?;
        }
        if version < 45 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            if !tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('scholar_jobs') WHERE name='next_attempt_at_ms')",[],|row|row.get::<_,bool>(0)).map_err(database_error)? {
                tx.execute_batch("ALTER TABLE scholar_jobs ADD COLUMN next_attempt_at_ms INTEGER NOT NULL DEFAULT 0;").map_err(database_error)?;
            }
            tx.execute_batch(
                "CREATE INDEX IF NOT EXISTS scholar_jobs_by_retry ON scholar_jobs(status,next_attempt_at_ms);
                 PRAGMA user_version = 45;"
            ).map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 46 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            if !tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('scholar_host_backoff') WHERE name='failures')",[],|row|row.get::<_,bool>(0)).map_err(database_error)? {
                tx.execute_batch("ALTER TABLE scholar_host_backoff ADD COLUMN failures INTEGER NOT NULL DEFAULT 1;").map_err(database_error)?;
            }
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS wikipedia_response_cache (
                    request_key TEXT PRIMARY KEY, request_kind TEXT NOT NULL,
                    response_json TEXT NOT NULL, expires_at_ms INTEGER NOT NULL
                 );
                      CREATE TABLE IF NOT EXISTS research_page_cache (
                          url TEXT PRIMARY KEY,content_type TEXT NOT NULL,body TEXT NOT NULL,
                          expires_at_ms INTEGER NOT NULL
                      );
                 PRAGMA user_version = 46;",
            )
            .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 47 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                    "CREATE TABLE IF NOT EXISTS research_sources (
                          id INTEGER PRIMARY KEY,job_id TEXT NOT NULL REFERENCES scholar_jobs(id),
                          title TEXT NOT NULL,url TEXT NOT NULL,content_hash TEXT NOT NULL,body TEXT NOT NULL,
                          harvest_cursor INTEGER NOT NULL DEFAULT 0,claimed_until_ms INTEGER NOT NULL DEFAULT 0,
                          next_attempt_at_ms INTEGER NOT NULL DEFAULT 0,last_error TEXT,created_at TEXT NOT NULL,
                          UNIQUE(url,content_hash)
                      );
                     CREATE TABLE IF NOT EXISTS research_claims (
                          id INTEGER PRIMARY KEY,source_id INTEGER NOT NULL REFERENCES research_sources(id),
                          subject TEXT NOT NULL,relation TEXT NOT NULL,object TEXT NOT NULL,
                          subject_key TEXT NOT NULL,relation_key TEXT NOT NULL,
                          evidence_text TEXT NOT NULL,qualifications TEXT NOT NULL,
                          extraction_confidence REAL NOT NULL CHECK(extraction_confidence BETWEEN 0 AND 1),
                          created_at TEXT NOT NULL,UNIQUE(source_id,subject,relation,object,evidence_text)
                      );
                     CREATE INDEX IF NOT EXISTS research_claims_by_subject ON research_claims(subject_key,relation_key);
                     CREATE TABLE IF NOT EXISTS research_questions (
                          id INTEGER PRIMARY KEY,source_id INTEGER NOT NULL REFERENCES research_sources(id),
                          question TEXT NOT NULL,question_key TEXT NOT NULL UNIQUE,evidence_text TEXT NOT NULL,
                          depth INTEGER NOT NULL,status TEXT NOT NULL DEFAULT 'pending'
                                CHECK(status IN ('pending','queued','researched','unanswered')),
                          scholar_job_id TEXT REFERENCES scholar_jobs(id),created_at TEXT NOT NULL
                      );
                     CREATE TABLE IF NOT EXISTS research_question_sources (
                          question_id INTEGER NOT NULL REFERENCES research_questions(id),
                          source_id INTEGER NOT NULL REFERENCES research_sources(id),evidence_text TEXT NOT NULL,
                          PRIMARY KEY(question_id,source_id)
                      );
                     CREATE TABLE IF NOT EXISTS research_evidence_links (
                          left_claim_id INTEGER NOT NULL REFERENCES research_claims(id),
                          right_claim_id INTEGER NOT NULL REFERENCES research_claims(id),
                          relation TEXT NOT NULL CHECK(relation IN ('supports','contradicts','qualifies','unrelated','insufficient')),
                          rationale TEXT NOT NULL,reviewer_model_id TEXT NOT NULL,
                          next_attempt_at_ms INTEGER NOT NULL DEFAULT 0,created_at TEXT NOT NULL,
                          PRIMARY KEY(left_claim_id,right_claim_id),CHECK(left_claim_id < right_claim_id)
                      );
                      PRAGMA user_version = 47;"
                ).map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 48 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS research_claim_relations (
                    left_claim_id INTEGER NOT NULL REFERENCES research_claims(id),
                    right_claim_id INTEGER NOT NULL REFERENCES research_claims(id),
                    relation TEXT NOT NULL CHECK(relation IN ('same_subject','related_facts')),
                    rationale TEXT NOT NULL,
                    reviewer_model_id TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                    PRIMARY KEY(left_claim_id,right_claim_id),
                    CHECK(left_claim_id < right_claim_id)
                 );
                 CREATE INDEX IF NOT EXISTS research_claim_relations_by_right
                    ON research_claim_relations(right_claim_id);
                 PRAGMA user_version = 48;",
            )
            .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 49 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS research_claim_terms (
                    term TEXT NOT NULL,
                    claim_id INTEGER NOT NULL REFERENCES research_claims(id) ON DELETE CASCADE,
                    PRIMARY KEY(term,claim_id)
                 );
                 CREATE INDEX IF NOT EXISTS research_claim_terms_by_claim
                    ON research_claim_terms(claim_id,term);
                 PRAGMA user_version = 49;",
            )
            .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 50 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            for (column, definition) in [
                ("source_scope", "TEXT NOT NULL DEFAULT 'legacy_excerpt'"),
                ("input_characters", "INTEGER NOT NULL DEFAULT 0"),
                ("retained_characters", "INTEGER NOT NULL DEFAULT 0"),
                ("truncated", "INTEGER NOT NULL DEFAULT 0"),
                ("provider", "TEXT NOT NULL DEFAULT 'unknown'"),
                ("query", "TEXT"),
                ("published_at", "TEXT"),
                ("capture_status", "TEXT NOT NULL DEFAULT 'excerpted'"),
            ] {
                let exists: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('research_sources') WHERE name = ?1)",
                        [column],
                        |row| row.get(0),
                    )
                    .map_err(database_error)?;
                if !exists {
                    tx.execute_batch(&format!(
                        "ALTER TABLE research_sources ADD COLUMN {column} {definition};"
                    ))
                    .map_err(database_error)?;
                }
            }
            tx.execute(
                "UPDATE research_sources SET input_characters=length(body),
                    retained_characters=length(body)
                 WHERE input_characters=0 AND retained_characters=0",
                [],
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 50)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 51 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            for (table, column) in [
                ("research_claims", "evidence_byte_start"),
                ("research_claims", "evidence_byte_end"),
                ("research_questions", "evidence_byte_start"),
                ("research_questions", "evidence_byte_end"),
                ("research_question_sources", "evidence_byte_start"),
                ("research_question_sources", "evidence_byte_end"),
            ] {
                let exists = table_has_column(&tx, table, column)?;
                if !exists {
                    tx.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} INTEGER;"))
                        .map_err(database_error)?;
                }
            }
            tx.pragma_update(None, "user_version", 51)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 52 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            for (column, definition) in [
                (
                    "source_archive_id",
                    "INTEGER REFERENCES research_sources(id)",
                ),
                ("evidence_byte_start", "INTEGER"),
                ("evidence_byte_end", "INTEGER"),
            ] {
                let exists: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('scholar_findings') WHERE name = ?1)",
                        [column],
                        |row| row.get(0),
                    )
                    .map_err(database_error)?;
                if !exists {
                    tx.execute_batch(&format!(
                        "ALTER TABLE scholar_findings ADD COLUMN {column} {definition};"
                    ))
                    .map_err(database_error)?;
                }
            }
            tx.pragma_update(None, "user_version", 52)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 53 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS scholar_finding_claim_links (
                    finding_id INTEGER NOT NULL REFERENCES scholar_findings(id),
                    claim_id INTEGER NOT NULL REFERENCES semantic_claims(id),
                    relation TEXT NOT NULL CHECK (relation IN ('supports', 'contradicts')),
                    rationale TEXT NOT NULL,
                    assessment_method TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
                    PRIMARY KEY(finding_id, claim_id, relation)
                 );
                 CREATE INDEX IF NOT EXISTS scholar_finding_claim_links_by_claim
                    ON scholar_finding_claim_links(claim_id, relation);
                 PRAGMA user_version = 53;",
            )
            .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 54 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            for (table, column, definition) in [
                ("semantic_claims", "attribution", "TEXT"),
                (
                    "semantic_claims",
                    "polarity",
                    "TEXT NOT NULL DEFAULT 'unknown' CHECK (polarity IN ('asserted','negated','unknown'))",
                ),
                (
                    "semantic_claims",
                    "modality",
                    "TEXT NOT NULL DEFAULT 'unknown' CHECK (modality IN ('factual','possible','necessary','conditional','unknown'))",
                ),
                ("scholar_findings", "attribution", "TEXT"),
                (
                    "scholar_findings",
                    "polarity",
                    "TEXT NOT NULL DEFAULT 'unknown' CHECK (polarity IN ('asserted','negated','unknown'))",
                ),
                (
                    "scholar_findings",
                    "modality",
                    "TEXT NOT NULL DEFAULT 'unknown' CHECK (modality IN ('factual','possible','necessary','conditional','unknown'))",
                ),
            ] {
                let exists = table_has_column(&tx, table, column)?;
                if !exists {
                    tx.execute_batch(&format!(
                        "ALTER TABLE {table} ADD COLUMN {column} {definition};"
                    ))
                    .map_err(database_error)?;
                }
            }
            tx.pragma_update(None, "user_version", 54)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 55 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            let definition = table_definition(&tx, "scholar_finding_claim_links")?;
            if !definition.is_some_and(|sql| sql.contains("'qualifies'")) {
                tx.execute_batch(
                    "CREATE TABLE scholar_finding_claim_links_v55 (
                        finding_id INTEGER NOT NULL REFERENCES scholar_findings(id),
                        claim_id INTEGER NOT NULL REFERENCES semantic_claims(id),
                        relation TEXT NOT NULL CHECK (relation IN ('supports','contradicts','qualifies','does_not_address')),
                        rationale TEXT NOT NULL,
                        assessment_method TEXT NOT NULL,
                        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
                        PRIMARY KEY(finding_id, claim_id, relation)
                    );
                    INSERT INTO scholar_finding_claim_links_v55
                        (finding_id,claim_id,relation,rationale,assessment_method,created_at)
                    SELECT finding_id,claim_id,relation,rationale,assessment_method,created_at
                    FROM scholar_finding_claim_links;
                    DROP TABLE scholar_finding_claim_links;
                    ALTER TABLE scholar_finding_claim_links_v55
                        RENAME TO scholar_finding_claim_links;
                    CREATE INDEX scholar_finding_claim_links_by_claim
                        ON scholar_finding_claim_links(claim_id, relation);",
                )
                .map_err(database_error)?;
            }
            tx.pragma_update(None, "user_version", 55)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 56 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            for (column, definition) in [
                (
                    "decision_method",
                    "TEXT NOT NULL DEFAULT 'deterministic_policy' CHECK (decision_method IN ('deterministic_policy','human','ai'))",
                ),
                ("model_id", "TEXT"),
                ("model_version", "TEXT"),
            ] {
                let exists: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('policy_decisions') WHERE name = ?1)",
                        [column],
                        |row| row.get(0),
                    )
                    .map_err(database_error)?;
                if !exists {
                    tx.execute_batch(&format!(
                        "ALTER TABLE policy_decisions ADD COLUMN {column} {definition};"
                    ))
                    .map_err(database_error)?;
                }
            }
            tx.pragma_update(None, "user_version", 56)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 57 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            let has_scope: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('entity_alias_links') WHERE name='identity_scope')",
                    [],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            if !has_scope {
                tx.execute_batch("ALTER TABLE entity_alias_links ADD COLUMN identity_scope TEXT;")
                    .map_err(database_error)?;
            }
            tx.execute_batch(
                "DROP VIEW IF EXISTS entity_equivalences;
                 DROP VIEW IF EXISTS entity_alias_candidates;
                 DROP VIEW IF EXISTS active_entity_aliases;
                 CREATE VIEW active_entity_aliases AS
                    SELECT left_name,right_name FROM entity_alias_links l
                    WHERE l.identity_scope IS NULL AND (l.source_finding_id IS NULL OR EXISTS (
                        SELECT 1 FROM scholar_findings f WHERE f.id=l.source_finding_id
                          AND (f.status='needs_review' OR (f.status='promoted' AND EXISTS (
                            SELECT 1 FROM semantic_claims c WHERE c.id=f.semantic_claim_id
                              AND c.retracted_by IS NULL AND c.status IN ('active','uncertain'))))));
                 CREATE VIEW entity_alias_candidates AS
                    SELECT left_name,right_name FROM active_entity_aliases
                    UNION SELECT right_name,left_name FROM active_entity_aliases;
                 CREATE VIEW entity_equivalences AS
                    SELECT a.left_name,a.right_name FROM entity_alias_candidates a
                    WHERE (SELECT COUNT(DISTINCT b.right_name) FROM entity_alias_candidates b
                           WHERE b.left_name=a.left_name)=1
                      AND (SELECT COUNT(DISTINCT b.right_name) FROM entity_alias_candidates b
                           WHERE b.left_name=a.right_name)=1;",
            )
            .map_err(database_error)?;
            tx.pragma_update(None, "user_version", 57)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 58 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            for table in ["semantic_claims", "scholar_findings"] {
                for column in ["geographic_scope", "context_scope"] {
                    let exists = table_has_column(&tx, table, column)?;
                    if !exists {
                        tx.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} TEXT;"))
                            .map_err(database_error)?;
                    }
                }
            }
            tx.pragma_update(None, "user_version", 58)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 59 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            let definition = table_definition(&tx, "scholar_finding_claim_links")?;
            if !definition.is_some_and(|sql| sql.contains("'unresolved'")) {
                tx.execute_batch(
                    "CREATE TABLE scholar_finding_claim_links_v59 (
                        finding_id INTEGER NOT NULL REFERENCES scholar_findings(id),
                        claim_id INTEGER NOT NULL REFERENCES semantic_claims(id),
                        relation TEXT NOT NULL CHECK (relation IN ('supports','contradicts','qualifies','does_not_address','unresolved')),
                        rationale TEXT NOT NULL,
                        assessment_method TEXT NOT NULL,
                        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
                        PRIMARY KEY(finding_id, claim_id, relation)
                    );
                    INSERT INTO scholar_finding_claim_links_v59
                        (finding_id,claim_id,relation,rationale,assessment_method,created_at)
                    SELECT finding_id,claim_id,relation,rationale,assessment_method,created_at
                    FROM scholar_finding_claim_links;
                    DROP TABLE scholar_finding_claim_links;
                    ALTER TABLE scholar_finding_claim_links_v59
                        RENAME TO scholar_finding_claim_links;
                    CREATE INDEX scholar_finding_claim_links_by_claim
                        ON scholar_finding_claim_links(claim_id, relation);",
                )
                .map_err(database_error)?;
            }
            tx.pragma_update(None, "user_version", 59)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 60 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS scholar_task_targets (
                    job_id TEXT PRIMARY KEY REFERENCES scholar_jobs(id) ON DELETE CASCADE,
                    original_question TEXT NOT NULL,
                    entity_constraints_json TEXT NOT NULL DEFAULT '[]',
                    scope_constraints_json TEXT NOT NULL DEFAULT '[]',
                    evidence_gap TEXT NOT NULL,
                    acceptable_evidence_json TEXT NOT NULL DEFAULT '[\"direct source passage that addresses the question\"]',
                    stop_reason TEXT,
                    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
                 );
                 INSERT OR IGNORE INTO scholar_task_targets(job_id,original_question,evidence_gap)
                    SELECT id,question,question FROM scholar_jobs;
                 CREATE TRIGGER IF NOT EXISTS scholar_job_target_after_insert
                 AFTER INSERT ON scholar_jobs BEGIN
                    INSERT OR IGNORE INTO scholar_task_targets(job_id,original_question,evidence_gap)
                    VALUES (NEW.id,NEW.question,NEW.question);
                 END;
                 PRAGMA user_version = 60;",
            )
            .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 61 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "UPDATE semantic_claims SET status='challenged',
                    checked_at=strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                    last_reviewed_at=strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                    next_review_at=NULL
                 WHERE status IN ('active','uncertain') AND retracted_by IS NULL
                   AND id IN (
                     SELECT DISTINCT f.semantic_claim_id
                     FROM scholar_findings f
                     JOIN scholar_source_assessments a ON a.finding_id=f.id
                     JOIN scholar_source_origin_reviews automatic_origin
                       ON automatic_origin.finding_id=f.id AND automatic_origin.review_method='automatic'
                     WHERE f.status='promoted' AND f.decision_reason LIKE 'automatically accepted after two directly supporting%'
                       AND a.independence_verified=1
                       AND NOT EXISTS (SELECT 1 FROM scholar_source_origin_reviews human_origin
                                       WHERE human_origin.finding_id=f.id AND human_origin.review_method='human')
                       AND NOT EXISTS (SELECT 1 FROM claim_evidence_links l
                                       JOIN claim_evidence e ON e.id=l.evidence_id
                                       WHERE l.claim_id=f.semantic_claim_id
                                         AND l.stance='supports' AND e.source_kind IN ('user_observation','manual'))
                       AND NOT EXISTS (SELECT 1 FROM scholar_findings independently_reviewed
                                       JOIN scholar_source_assessments verified
                                         ON verified.finding_id=independently_reviewed.id
                                       JOIN scholar_source_origin_reviews human_origin
                                         ON human_origin.finding_id=independently_reviewed.id
                                         AND human_origin.review_method='human'
                                       WHERE independently_reviewed.semantic_claim_id=f.semantic_claim_id
                                         AND independently_reviewed.status='promoted'
                                         AND verified.independence_verified=1)
                   );

                 UPDATE scholar_findings SET status='needs_review'
                 WHERE status='promoted' AND decision_reason LIKE 'automatically accepted after two directly supporting%'
                   AND EXISTS (SELECT 1 FROM scholar_source_origin_reviews automatic_origin
                               WHERE automatic_origin.finding_id=scholar_findings.id
                                 AND automatic_origin.review_method='automatic')
                   AND NOT EXISTS (SELECT 1 FROM scholar_source_origin_reviews human_origin
                                   WHERE human_origin.finding_id=scholar_findings.id
                                     AND human_origin.review_method='human');

                 UPDATE scholar_source_assessments SET independence_verified=0,
                    gate_status=CASE WHEN gate_status='eligible' THEN 'awaiting_corroboration' ELSE gate_status END,
                    reason=substr(reason,1,800) || '; automatic domain-based origin verification withdrawn: distinct hosts alone do not prove independent reporting'
                 WHERE independence_verified=1
                   AND EXISTS (SELECT 1 FROM scholar_source_origin_reviews automatic_origin
                               WHERE automatic_origin.finding_id=scholar_source_assessments.finding_id
                                 AND automatic_origin.review_method='automatic')
                   AND NOT EXISTS (SELECT 1 FROM scholar_source_origin_reviews human_origin
                                   WHERE human_origin.finding_id=scholar_source_assessments.finding_id
                                     AND human_origin.review_method='human');
                 PRAGMA user_version = 61;",
            )
            .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 62 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "UPDATE scholar_source_assessments SET direct_support=0,
                    gate_status='unsupported',
                    reason='plural claim exceeds the exact finding evidence: one example does not establish multiple options'
                 WHERE direct_support=1 AND finding_id IN (
                   SELECT f.id FROM scholar_findings f
                   WHERE (
                     lower(f.relation || ' ' || f.object) LIKE '%variety of%'
                     OR lower(f.relation || ' ' || f.object) LIKE '%range of%'
                     OR lower(f.relation || ' ' || f.object) LIKE '%various%'
                     OR lower(f.relation || ' ' || f.object) LIKE '%multiple%'
                     OR lower(f.relation || ' ' || f.object) LIKE '%several%'
                     OR lower(f.relation || ' ' || f.object) LIKE '%numerous%'
                     OR lower(f.relation || ' ' || f.object) LIKE '%different%'
                     OR lower(f.relation || ' ' || f.object) LIKE '%both%'
                   )
                   AND NOT (
                     lower(f.evidence_text) LIKE '%variety of%'
                     OR lower(f.evidence_text) LIKE '%range of%'
                     OR lower(f.evidence_text) LIKE '%various%'
                     OR lower(f.evidence_text) LIKE '%multiple%'
                     OR lower(f.evidence_text) LIKE '%several%'
                     OR lower(f.evidence_text) LIKE '%numerous%'
                     OR lower(f.evidence_text) LIKE '%different%'
                     OR lower(f.evidence_text) LIKE '%both%'
                     OR (instr(f.evidence_text, ',') > 0
                         AND (instr(lower(f.evidence_text), ' and ') > 0
                              OR instr(lower(f.evidence_text), ' or ') > 0))
                     OR length(f.evidence_text) - length(replace(f.evidence_text, char(10) || '* ', '')) >= 6
                     OR length(f.evidence_text) - length(replace(f.evidence_text, char(10) || '- ', '')) >= 6
                     OR length(f.evidence_text) - length(replace(f.evidence_text, char(10) || '• ', '')) >= 8
                   )
                 );
                 PRAGMA user_version = 62;",
            )
            .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 63 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            // Different strings cannot establish different meanings. Reopen
            // only links created by the old lexical decision, keeping reviewed
            // relationships and every source claim intact.
            tx.execute_batch(
                "UPDATE scholar_finding_claim_links
                 SET relation='unresolved',
                     rationale='same established subject with different relation wording; semantic review must decide whether the propositions address the same property',
                     assessment_method='relation_requires_semantic_review'
                 WHERE relation='does_not_address'
                   AND assessment_method='deterministic_distinct_relation';
                 PRAGMA user_version=63;",
            ).map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 65 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS scout_work_state (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    next_stage INTEGER NOT NULL DEFAULT 0 CHECK (next_stage BETWEEN 0 AND 4)
                 );
                 INSERT OR IGNORE INTO scout_work_state(id,next_stage) VALUES(1,0);
                 CREATE TABLE IF NOT EXISTS research_assertion_checks (
                    claim_id INTEGER PRIMARY KEY REFERENCES research_claims(id),
                    question TEXT,
                    selection_reason TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending','queued','researched','unanswered','skipped')),
                    scholar_job_id TEXT REFERENCES scholar_jobs(id),
                    last_admission_reason TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS research_assertion_checks_by_status
                    ON research_assertion_checks(status, claim_id);
                 PRAGMA user_version = 65;",
            )
            .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 66 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            if !table_has_column(&tx, "scholar_findings", "review_caveats_json")? {
                tx.execute(
                    "ALTER TABLE scholar_findings ADD COLUMN review_caveats_json TEXT NOT NULL DEFAULT '[]'",
                    [],
                )
                .map_err(database_error)?;
            }
            tx.pragma_update(None, "user_version", 66)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 67 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            let definition = table_definition(&tx, "scout_investigations")?.unwrap_or_default();
            if !definition.contains("'assertion'") {
                tx.execute_batch(
                    "ALTER TABLE scout_investigations RENAME TO scout_investigations_before_assertion;
                     CREATE TABLE scout_investigations (
                        id INTEGER PRIMARY KEY,
                        trigger_kind TEXT NOT NULL CHECK (trigger_kind IN ('challenged', 'uncertain', 'sparse', 'stale', 'expansion', 'model_probe', 'assertion')),
                        topic_key TEXT NOT NULL,
                        trigger_ref TEXT NOT NULL,
                        question TEXT NOT NULL,
                        scholar_job_id TEXT NOT NULL REFERENCES scholar_jobs(id),
                        selection_reason TEXT NOT NULL,
                        created_at TEXT NOT NULL
                     );
                     INSERT INTO scout_investigations
                        (id,trigger_kind,topic_key,trigger_ref,question,scholar_job_id,selection_reason,created_at)
                     SELECT id,trigger_kind,topic_key,trigger_ref,question,scholar_job_id,selection_reason,created_at
                     FROM scout_investigations_before_assertion;
                     DROP TABLE scout_investigations_before_assertion;
                     CREATE INDEX scout_investigations_by_topic
                        ON scout_investigations(trigger_kind, topic_key, id DESC);
                     CREATE INDEX scout_investigations_by_created
                        ON scout_investigations(created_at);",
                )
                .map_err(database_error)?;
            }
            tx.pragma_update(None, "user_version", 67)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 68 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            let definition =
                table_definition(&tx, "research_assertion_checks")?.unwrap_or_default();
            if !definition.contains("'verified'") {
                tx.execute_batch(
                    "ALTER TABLE research_assertion_checks
                        RENAME TO research_assertion_checks_before_outcomes;
                     CREATE TABLE research_assertion_checks (
                        claim_id INTEGER PRIMARY KEY REFERENCES research_claims(id),
                        question TEXT,
                        selection_reason TEXT NOT NULL,
                        status TEXT NOT NULL DEFAULT 'pending'
                            CHECK (status IN ('pending','queued','proposed','reviewed',
                                'waiting_for_evidence','verified','unanswered','skipped')),
                        scholar_job_id TEXT REFERENCES scholar_jobs(id),
                        last_admission_reason TEXT,
                        created_at TEXT NOT NULL,
                        updated_at TEXT NOT NULL
                     );
                     INSERT INTO research_assertion_checks
                        (claim_id,question,selection_reason,status,scholar_job_id,last_admission_reason,created_at,updated_at)
                     SELECT old.claim_id,old.question,old.selection_reason,
                        CASE
                          WHEN j.status='promoted' OR EXISTS(SELECT 1 FROM scholar_findings f
                               WHERE f.job_id=old.scholar_job_id AND f.status='promoted') THEN 'verified'
                          WHEN EXISTS(SELECT 1 FROM scholar_findings f
                               JOIN scholar_source_assessments a ON a.finding_id=f.id
                               WHERE f.job_id=old.scholar_job_id AND a.gate_status='awaiting_corroboration') THEN 'waiting_for_evidence'
                          WHEN EXISTS(SELECT 1 FROM scholar_findings f
                               WHERE f.job_id=old.scholar_job_id AND f.status='needs_review') THEN 'proposed'
                          WHEN EXISTS(SELECT 1 FROM scholar_findings f
                               WHERE f.job_id=old.scholar_job_id AND f.status='rejected') THEN 'reviewed'
                          WHEN j.status='needs_review' THEN 'proposed'
                          WHEN j.status='no_result' OR old.status='unanswered' THEN 'unanswered'
                          WHEN old.question IS NULL OR old.status='skipped' THEN 'skipped'
                          WHEN old.status='pending' THEN 'pending'
                          ELSE 'queued'
                        END,
                        old.scholar_job_id,old.last_admission_reason,old.created_at,old.updated_at
                     FROM research_assertion_checks_before_outcomes old
                     LEFT JOIN scholar_jobs j ON j.id=old.scholar_job_id;
                     DROP TABLE research_assertion_checks_before_outcomes;
                     CREATE INDEX research_assertion_checks_by_status
                        ON research_assertion_checks(status, claim_id);",
                )
                .map_err(database_error)?;
            }
            tx.pragma_update(None, "user_version", 68)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        if version < 69 {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            if !table_has_column(&tx, "research_assertion_checks", "derivation_depth")? {
                tx.execute(
                    "ALTER TABLE research_assertion_checks ADD COLUMN derivation_depth INTEGER NOT NULL DEFAULT 0 CHECK (derivation_depth BETWEEN 0 AND 3)",
                    [],
                )
                .map_err(database_error)?;
            }
            tx.pragma_update(None, "user_version", 69)
                .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        Ok(())
    }
}
