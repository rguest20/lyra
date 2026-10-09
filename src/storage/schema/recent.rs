use super::*;

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
        Ok(())
    }
}
