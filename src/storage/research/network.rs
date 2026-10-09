use super::*;
pub(crate) const MEDIAWIKI_TRANSIENT_ERROR: &str = "MediaWiki API returned a retryable error";

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
    pub fn cached_source_page(&self, url: &str) -> io::Result<Option<(String, String)>> {
        self.connection
            .query_row(
                "SELECT content_type,body FROM research_page_cache WHERE url=?1
            AND expires_at_ms>CAST(strftime('%s','now') AS INTEGER)*1000",
                [url],
                |row| Ok((row.get(0)?, row.get(1)?)),
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
        let now: i64 = self
            .connection
            .query_row(
                "SELECT CAST(strftime('%s','now') AS INTEGER)*1000",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
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
        let mut store = LearningStore::open(Path::new(":memory:")).unwrap();
        let now: i64 = store
            .connection
            .query_row(
                "SELECT CAST(strftime('%s','now') AS INTEGER)*1000",
                [],
                |row| row.get(0),
            )
            .unwrap();
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
        let mut store = LearningStore::open(Path::new(":memory:")).unwrap();
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
        let store = LearningStore::open(Path::new(":memory:")).unwrap();
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
        assert!(cached.1.contains("third useful detail"));
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
        let mut store = LearningStore::open(Path::new(":memory:")).unwrap();
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
        let mut store = LearningStore::open(Path::new(":memory:")).unwrap();
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
        let store = LearningStore::open(Path::new(":memory:")).unwrap();
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
