//! Shared host reservations, caches and Wikipedia request accounting.
use super::*;
use sha2::{Digest, Sha256};
pub(crate) const MEDIAWIKI_TRANSIENT_ERROR: &str = "MediaWiki API returned a retryable error";

impl CachedSitePolicy {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            status: row.get(0)?,
            content_type: row.get(1)?,
            body: row.get(2)?,
        })
    }
}

impl CachedSourcePage {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            content_type: row.get(0)?,
            body: row.get(1)?,
        })
    }
}

fn configured_limit(name: &str, default: i64, maximum: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| (1..=maximum).contains(value))
        .unwrap_or(default)
}

pub(crate) fn wikipedia_interval_ms() -> i64 {
    configured_limit("SCHOLAR_WIKIPEDIA_INTERVAL_SECONDS", 10, 600).max(10) * 1000
}

pub(crate) fn wikipedia_limits() -> (i64, i64) {
    (
        configured_limit("SCHOLAR_WIKIPEDIA_REQUESTS_PER_HOUR", 60, 360),
        configured_limit("SCHOLAR_WIKIPEDIA_REQUESTS_PER_DAY", 200, 8640),
    )
}

pub(super) fn check_wikipedia_budget(
    tx: &Transaction<'_>,
    host: &str,
    now: i64,
    limits: (i64, i64),
) -> io::Result<()> {
    if host != "en.wikipedia.org" {
        return Ok(());
    }
    for (window, limit, label) in [
        (3_600_000, limits.0, "hourly"),
        (86_400_000, limits.1, "daily"),
    ] {
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM host_request_log WHERE host='en.wikipedia.org' AND scheduled_at_ms >= ?1",
            [now.saturating_sub(window)], |row| row.get(0),
        ).map_err(database_error)?;
        if count >= limit {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                format!("Wikipedia {label} API request budget exhausted"),
            ));
        }
    }
    Ok(())
}

impl LearningStore {
    pub(crate) fn cached_source_page(&self, url: &str) -> io::Result<Option<CachedSourcePage>> {
        self.connection
            .query_row(
                "SELECT content_type,body FROM research_page_cache WHERE url=?1
            AND expires_at_ms>CAST(strftime('%s','now') AS INTEGER)*1000",
                [url],
                CachedSourcePage::from_row,
            )
            .optional()
            .map_err(database_error)
    }

    pub fn cache_source_page(&self, url: &str, content_type: &str, body: &str) -> io::Result<()> {
        if body.len() > 1_000_000 {
            return Ok(());
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(database_error)?;
        tx.execute("DELETE FROM research_page_cache WHERE expires_at_ms<=CAST(strftime('%s','now') AS INTEGER)*1000",[]).map_err(database_error)?;
        tx.execute("INSERT INTO research_page_cache(url,content_type,body,expires_at_ms)
            VALUES (?1,?2,?3,CAST(strftime('%s','now') AS INTEGER)*1000+3600000)
            ON CONFLICT(url) DO UPDATE SET content_type=excluded.content_type,body=excluded.body,expires_at_ms=excluded.expires_at_ms",params![url,content_type,body]).map_err(database_error)?;
        tx.execute("DELETE FROM research_page_cache WHERE url NOT IN (SELECT url FROM research_page_cache ORDER BY expires_at_ms DESC LIMIT 64)",[]).map_err(database_error)?;
        tx.commit().map_err(database_error)
    }

    pub fn research_network_usage(&self) -> io::Result<serde_json::Value> {
        let now = super::sql_now_millis(&self.connection)?;
        let count = |window| {
            self.connection.query_row("SELECT COUNT(*) FROM host_request_log WHERE host='en.wikipedia.org' AND scheduled_at_ms>=?1",[now-window],|row|row.get::<_,i64>(0)).map_err(database_error)
        };
        let pages: i64 = self.connection.query_row("SELECT COUNT(*) FROM host_request_log WHERE request_kind IN ('wikipedia_page_content','external_page_content') AND scheduled_at_ms>=?1",[now-86_400_000],|row|row.get(0)).map_err(database_error)?;
        let (hour, day) = wikipedia_limits();
        let backoff: i64 = self.connection.query_row("SELECT COALESCE(MAX(retry_until_ms),0) FROM scholar_host_backoff WHERE host='en.wikipedia.org'",[],|row|row.get(0)).map_err(database_error)?;
        Ok(
            serde_json::json!({"wikipedia_requests_last_hour":count(3_600_000)?,"wikipedia_hourly_limit":hour,
            "wikipedia_requests_last_day":count(86_400_000)?,"wikipedia_daily_limit":day,
            "shared_page_fetches_last_day":pages,"shared_page_fetch_limit":null,"wikipedia_backoff_until_ms":backoff,
            "scout":self.scout_queue_status()?}),
        )
    }

    pub fn cached_wikipedia_response(&self, key: &str) -> io::Result<Option<serde_json::Value>> {
        let body: Option<String> = self
            .connection
            .query_row(
                "SELECT response_json FROM wikipedia_response_cache WHERE request_key=?1
             AND expires_at_ms > CAST(strftime('%s','now') AS INTEGER)*1000",
                [key],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)?;
        body.map(|body| serde_json::from_str(&body).map_err(invalid_json))
            .transpose()
    }

    pub fn cache_wikipedia_response(
        &self,
        key: &str,
        kind: &str,
        response: &serde_json::Value,
    ) -> io::Result<()> {
        let ttl = if kind == "wikipedia_search" {
            3_600_000
        } else {
            86_400_000
        };
        let body = response.to_string();
        if body.len() > 2_000_000 {
            return Ok(());
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(database_error)?;
        tx.execute("DELETE FROM wikipedia_response_cache WHERE expires_at_ms <= CAST(strftime('%s','now') AS INTEGER)*1000",[]).map_err(database_error)?;
        tx.execute(
            "INSERT INTO wikipedia_response_cache(request_key,request_kind,response_json,expires_at_ms)
             VALUES (?1,?2,?3,CAST(strftime('%s','now') AS INTEGER)*1000+?4)
             ON CONFLICT(request_key) DO UPDATE SET response_json=excluded.response_json,expires_at_ms=excluded.expires_at_ms",
            params![key,kind,body,ttl],
        ).map_err(database_error)?;
        tx.execute("DELETE FROM wikipedia_response_cache WHERE request_key NOT IN
            (SELECT request_key FROM wikipedia_response_cache ORDER BY expires_at_ms DESC LIMIT 200)",[]).map_err(database_error)?;
        tx.commit().map_err(database_error)
    }

    pub(super) fn wikipedia_budget_retry_ms(&self, now: i64) -> io::Result<i64> {
        let limits = wikipedia_limits();
        let mut retry = now.saturating_add(60_000);
        for (window, limit, condition) in [
            (3_600_000, limits.0, "host='en.wikipedia.org'"),
            (86_400_000, limits.1, "host='en.wikipedia.org'"),
        ] {
            let count: i64 = self.connection.query_row(&format!(
                "SELECT COUNT(*) FROM host_request_log WHERE {condition} AND scheduled_at_ms >= ?1"
            ),[now.saturating_sub(window)],|row|row.get(0)).map_err(database_error)?;
            if count >= limit {
                let expires: i64 = self.connection.query_row(&format!(
                    "SELECT scheduled_at_ms+?2+1 FROM host_request_log WHERE {condition} AND scheduled_at_ms >= ?1
                     ORDER BY scheduled_at_ms LIMIT 1 OFFSET ?3"
                ),params![now.saturating_sub(window),window,count-limit],|row|row.get(0)).map_err(database_error)?;
                retry = retry.max(expires);
            }
        }
        Ok(retry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_fetches_have_no_shared_daily_cap_or_retry_delay() {
        let mut store = crate::test_support::in_memory_learning_store();
        let now = super::super::sql_now_millis(&store.connection).unwrap();
        for _ in 0..101 {
            store.connection.execute("INSERT INTO host_request_log(host,request_kind,scheduled_at_ms,completed_at_ms,outcome,lease_until_ms)
                VALUES ('past.example','external_page_content',?1,?1,'succeeded',?1)",[now-3_600_000]).unwrap();
        }
        assert_eq!(store.wikipedia_budget_retry_ms(now).unwrap(), now + 60_000);
        let usage = store.research_network_usage().unwrap();
        assert_eq!(usage["shared_page_fetches_last_day"], 101);
        assert!(usage["shared_page_fetch_limit"].is_null());
        assert!(
            store
                .reserve_host_request_with_interval("next.example", "external_page_content", 10_000)
                .is_ok()
        );
        assert!(
            store
                .reserve_host_request_with_interval(
                    "en.wikipedia.org",
                    "wikipedia_page_content",
                    10_000
                )
                .is_ok()
        );
    }

    #[test]
    fn jobs_deferred_by_removed_cap_resume_but_other_backoffs_remain() {
        let mut store = crate::test_support::in_memory_learning_store();
        let plan = crate::research::ResearchPlan {
            question: "Where is the Amazon rainforest?".into(),
            queries: vec!["Amazon rainforest".into()],
        };
        store
            .enqueue_research_plan("removed-cap", 1, &plan)
            .unwrap();
        let job = store
            .claim_conversation_research("removed-cap", 1)
            .unwrap()
            .unwrap();
        let future: i64 = store
            .connection
            .query_row(
                "SELECT CAST(strftime('%s','now') AS INTEGER)*1000+86400000",
                [],
                |row| row.get(0),
            )
            .unwrap();
        store
            .defer_scholar_job(
                &job.id,
                "Scholar reached the daily external page-fetch limit",
                future,
            )
            .unwrap();
        assert!(store.has_pending_user_research().unwrap());
        assert_eq!(
            store
                .claim_conversation_research("removed-cap", 1)
                .unwrap()
                .unwrap()
                .id,
            job.id
        );
        store
            .defer_scholar_job(
                &job.id,
                "Wikipedia daily API request budget exhausted",
                future,
            )
            .unwrap();
        assert!(!store.has_pending_user_research().unwrap());
        assert!(
            store
                .claim_conversation_research("removed-cap", 1)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn external_page_cache_retains_full_content_and_expires() {
        let store = crate::test_support::in_memory_learning_store();
        store
            .cache_source_page(
                "https://example.org/page",
                "text/plain",
                "One fact. Another fact. A third useful detail.",
            )
            .unwrap();
        let cached = store
            .cached_source_page("https://example.org/page")
            .unwrap()
            .unwrap();
        assert!(cached.body.contains("third useful detail"));
        store
            .connection
            .execute("UPDATE research_page_cache SET expires_at_ms=0", [])
            .unwrap();
        assert!(
            store
                .cached_source_page("https://example.org/page")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn mediawiki_throttle_keeps_actual_http_status_and_increases_backoff() {
        let mut store = crate::test_support::in_memory_learning_store();
        let first = store
            .reserve_host_request_with_interval("en.wikipedia.org", "wikipedia_search", 10_000)
            .unwrap();
        store
            .complete_host_request_with_retry_after(
                first.id,
                Some(200),
                None,
                Some(MEDIAWIKI_TRANSIENT_ERROR),
            )
            .unwrap();
        let (first_delay, failures): (i64, i64) = store
            .connection
            .query_row(
                "SELECT retry_until_ms-updated_at_ms,failures FROM scholar_host_backoff",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(first_delay, 60_000);
        assert_eq!(failures, 1);
        let recorded: i64 = store
            .connection
            .query_row(
                "SELECT response_status FROM host_request_log WHERE id=?1",
                [first.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(recorded, 200);
        assert_eq!(
            store
                .reserve_host_request_with_interval("en.wikipedia.org", "wikipedia_search", 10_000)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        store
            .connection
            .execute("UPDATE scholar_host_backoff SET retry_until_ms=0", [])
            .unwrap();
        let second = store
            .reserve_host_request_with_interval("en.wikipedia.org", "wikipedia_search", 10_000)
            .unwrap();
        store
            .complete_host_request_with_retry_after(
                second.id,
                Some(429),
                None,
                Some("rate limited"),
            )
            .unwrap();
        let delay: i64 = store
            .connection
            .query_row(
                "SELECT retry_until_ms-updated_at_ms FROM scholar_host_backoff",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(delay, 120_000);
    }

    #[test]
    fn api_budget_counts_searches_and_sections_not_only_page_fetches() {
        let mut store = crate::test_support::in_memory_learning_store();
        let tx = store.connection.transaction().unwrap();
        tx.execute(
            "INSERT INTO host_request_log(host,request_kind,scheduled_at_ms,outcome,lease_until_ms)
            VALUES ('en.wikipedia.org','wikipedia_search',10000000,'succeeded',10000000)",
            [],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO host_request_log(host,request_kind,scheduled_at_ms,outcome,lease_until_ms)
            VALUES ('en.wikipedia.org','wikipedia_sections',10010000,'succeeded',10010000)",
            [],
        )
        .unwrap();
        assert_eq!(
            check_wikipedia_budget(&tx, "en.wikipedia.org", 10020000, (2, 100))
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(check_wikipedia_budget(&tx, "en.wikipedia.org", 10020000, (3, 100)).is_ok());
        assert!(check_wikipedia_budget(&tx, "example.org", 10020000, (1, 1)).is_ok());
        assert!(check_wikipedia_budget(&tx, "en.wikipedia.org", 14000000, (2, 100)).is_ok());
    }

    #[test]
    fn cached_responses_expire_and_do_not_consume_requests() {
        let store = crate::test_support::in_memory_learning_store();
        let response = serde_json::json!({"query":{"pages":[]}});
        store
            .cache_wikipedia_response("fixture", "wikipedia_search", &response)
            .unwrap();
        assert_eq!(
            store.cached_wikipedia_response("fixture").unwrap(),
            Some(response)
        );
        let count: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM host_request_log", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        store
            .connection
            .execute("UPDATE wikipedia_response_cache SET expires_at_ms=0", [])
            .unwrap();
        assert!(
            store
                .cached_wikipedia_response("fixture")
                .unwrap()
                .is_none()
        );
    }
}

pub(crate) fn robots_block_key(path_query: &str) -> String {
    format!("{:x}", Sha256::digest(path_query.as_bytes()))
}

impl LearningStore {
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
            .query_map([sqlite_limit(limit.min(100))?], |row| {
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
                    | "searxng_search"
            )
        {
            return Err(invalid("unsupported Scholar host request"));
        }
        let now = super::unix_time_ms_i64("request timestamp too large")?;
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
            let after_completion = completed.map_or_else(
                || scheduled.saturating_add(15_000),
                |completed| completed.saturating_add(minimum_interval_ms),
            );
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
        let completed_at_ms = super::unix_time_ms_i64("request completion timestamp too large")?;
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

    pub(crate) fn cached_site_policy(
        &self,
        origin: &str,
        request_kind: &str,
    ) -> io::Result<Option<CachedSitePolicy>> {
        let row = self
            .connection
            .query_row(
                "SELECT response_status, content_type, body FROM scholar_site_policy_cache
                 WHERE origin = ?1 AND request_kind = ?2
                   AND expires_at_ms > CAST(strftime('%s', 'now') AS INTEGER) * 1000",
                params![origin, request_kind],
                CachedSitePolicy::from_row,
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
        let now = super::unix_time_ms_i64("policy timestamp too large")?;
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
}
