//! Storage regressions for network.
use super::*;

#[test]
fn wikipedia_search_attempt_keeps_query_and_outcome_across_restart() {
    let path = temp_path();
    let store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("test", 1, "What are the moons of Mars?")
        .unwrap();
    drop(store);
    let mut store = LearningStore::open(&path).unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .record_scholar_search_attempt(&job.id, "Mars moons", "no_result", Some(0), None)
        .unwrap();
    drop(store);
    let store = LearningStore::open(&path).unwrap();
    let attempts = store.scholar_search_attempts(&job.id).unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].provider, "wikipedia");
    assert_eq!(attempts[0].trigger_reason, "primary");
    assert_eq!(attempts[0].query, "Mars moons");
    assert_eq!(attempts[0].status, "no_result");
    let _ = std::fs::remove_file(path);
}

#[test]
fn serpapi_cooldown_and_discovery_candidates_survive_restart() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("test", 1, "Chicken soup ingredients")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    let reservation = store
        .reserve_serpapi_search(1_800_000, &job.id, "Chicken soup ingredients")
        .unwrap();
    store
        .complete_serpapi_search(reservation.id, Some(200), None)
        .unwrap();
    store
        .record_typed_search_candidate(
            &job.id,
            "serpapi",
            "Chicken soup ingredients",
            &scholar_search_candidate(
                "recipe",
                "Chicken soup",
                "https://example.org/chicken-soup",
                "Search result snippet is discovery only.",
                r#"{"source":"Example Kitchen","ingredients":["chicken","carrots"],"rating":4.5}"#,
            ),
        )
        .unwrap();
    let candidate_id = store.scholar_search_candidates(&job.id).unwrap()[0].id;
    store
        .record_scholar_candidate_fetch_result(
            candidate_id,
            &scholar_candidate_review(
                "fetched",
                "allowed",
                Some("robots and site policy allowed the fetch"),
                "fetched",
                Some("https://example.org/chicken-soup"),
                Some("Chicken soup contains chicken and carrots."),
            ),
        )
        .unwrap();
    store
        .record_scholar_candidate_provenance(
            candidate_id,
            r#"{"canonical_url":"https://example.org/chicken-soup","author":"A Cook","publisher":null,"published_at":null,"cited_urls":[]}"#,
        )
        .unwrap();
    store
        .record_serpapi_search_domains(
            reservation.id,
            &[
                ("www.Example.org.".to_owned(), 2),
                ("news.example.net".to_owned(), 1),
            ],
        )
        .unwrap();
    drop(store);

    let mut store = LearningStore::open(&path).unwrap();
    assert_eq!(
        store
            .reserve_serpapi_search(1_800_000, &job.id, "second query")
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    let candidates = store.scholar_search_candidates(&job.id).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].provider, "serpapi");
    assert_eq!(candidates[0].query, "Chicken soup ingredients");
    assert_eq!(candidates[0].result_type, "recipe");
    assert!(
        candidates[0]
            .page_provenance_json
            .as_deref()
            .unwrap()
            .contains("A Cook")
    );
    assert!(
        candidates[0]
            .structured_data_json
            .contains("Example Kitchen")
    );
    assert_eq!(candidates[0].status, "fetched");
    assert_eq!(candidates[0].policy_status, "allowed");
    assert_eq!(
        candidates[0].fetched_excerpt.as_deref(),
        Some("Chicken soup contains chicken and carrots.")
    );
    assert_eq!(candidates[0].url, "https://example.org/chicken-soup");
    let domains = store.serpapi_domain_history(50).unwrap();
    assert_eq!(domains.len(), 2);
    let example_domain = domains
        .iter()
        .find(|domain| domain.domain == "example.org")
        .unwrap();
    assert_eq!(example_domain.query, "Chicken soup ingredients");
    assert_eq!(example_domain.result_count, 2);
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn tavily_daily_reservations_enforce_the_persisted_call_cap() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    for _ in 0..25 {
        store
            .reserve_tavily_search_with_interval("basic", 0)
            .unwrap();
    }
    assert_eq!(
        store
            .reserve_tavily_search_with_interval("basic", 0)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_eq!(
        store
            .reserve_tavily_search_with_interval("unsupported", 0)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidData
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn tavily_advanced_retries_reserve_two_monthly_credits() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let historical = now - 25 * 60 * 60 * 1000;
    for _ in 0..374 {
        store
            .connection
            .execute(
                "INSERT INTO tavily_usage (scheduled_at_ms, search_depth, credits, outcome)
                 VALUES (?1, 'advanced', 2, 'succeeded')",
                [historical],
            )
            .unwrap();
    }
    store
        .connection
        .execute(
            "INSERT INTO tavily_usage (scheduled_at_ms, search_depth, credits, outcome)
             VALUES (?1, 'basic', 1, 'succeeded')",
            [historical],
        )
        .unwrap();
    assert_eq!(
        store
            .reserve_tavily_search_with_interval("advanced", 0)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock,
        "a two-credit advanced retry must respect the 750-credit monthly ceiling"
    );
    store
        .reserve_tavily_search_with_interval("basic", 0)
        .unwrap();
    let credits: i64 = store
        .connection
        .query_row("SELECT SUM(credits) FROM tavily_usage", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(credits, 750);
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn tavily_hourly_spacing_survives_restart() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .reserve_tavily_search_with_interval("basic", 3_600_000)
        .unwrap();
    drop(store);

    let mut reopened = LearningStore::open(&path).unwrap();
    assert!(
        reopened
            .last_scholar_provider_search_ms("tavily")
            .unwrap()
            .is_some()
    );
    assert_eq!(
        reopened
            .reserve_tavily_search_with_interval("basic", 3_600_000)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    drop(reopened);
    let _ = std::fs::remove_file(path);
}

#[test]
fn robots_denials_skip_only_the_recorded_path_and_expire_across_restarts() {
    let path = temp_path();
    let store = LearningStore::open(&path).unwrap();
    store
        .remember_robots_block(
            "example.org",
            "/private/article?view=full",
            "/private/article",
            "robots.txt disallows this source path",
            false,
        )
        .unwrap();
    assert!(
        store
            .robots_blocked("example.org", "/private/article?view=full")
            .unwrap()
    );
    assert!(
        !store
            .robots_blocked("example.org", "/public/article")
            .unwrap()
    );
    store
        .remember_robots_block(
            "closed.example.org",
            "/first",
            "/first",
            "robots.txt is access denied",
            true,
        )
        .unwrap();
    assert!(
        store
            .robots_blocked("closed.example.org", "/another")
            .unwrap()
    );
    drop(store);
    let store = LearningStore::open(&path).unwrap();
    let blocks = store.scholar_robots_blocks(10).unwrap();
    assert_eq!(blocks.len(), 2);
    assert!(
        blocks
            .iter()
            .any(|block| block.display_path == "/private/article")
    );
    assert!(blocks.iter().any(|block| block.display_path == "/*"));
    store
        .connection
        .execute(
            "UPDATE scholar_robots_blocks SET expires_at = '2000-01-01T00:00:00.000Z'",
            [],
        )
        .unwrap();
    assert!(
        !store
            .robots_blocked("example.org", "/private/article?view=full")
            .unwrap()
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn robots_cache_migration_imports_prior_denied_candidates() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store
        .enqueue_research("robots-migration", 1, "topic")
        .unwrap();
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .record_typed_search_candidate(
            &job.id,
            "serpapi",
            "topic",
            &scholar_search_candidate(
                "organic_result",
                "Private article",
                "https://example.org/private?view=full",
                "topic",
                "{}",
            ),
        )
        .unwrap();
    let candidate = store.scholar_search_candidates(&job.id).unwrap().remove(0);
    store
        .record_scholar_candidate_fetch_result(
            candidate.id,
            &scholar_candidate_review(
                "skipped",
                "disallowed",
                Some("robots.txt disallows the candidate path for CommunityLabScholar"),
                "skipped",
                None,
                None,
            ),
        )
        .unwrap();
    store
        .connection
        .execute_batch("PRAGMA user_version = 28;")
        .unwrap();
    drop(store);
    let store = LearningStore::open(&path).unwrap();
    assert!(
        store
            .robots_blocked("example.org", "/private?view=full")
            .unwrap()
    );
    assert_eq!(store.scholar_robots_blocks(10).unwrap().len(), 1);
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn scholar_http_slots_enforce_global_and_per_host_caps_and_recover_expired_leases() {
    let path = temp_path();
    let mut first = LearningStore::open(&path).unwrap();
    let mut second = LearningStore::open(&path).unwrap();
    let a = first
        .reserve_host_request_with_interval("one.example", "tavily_search", 10_000)
        .unwrap();
    let b = second
        .reserve_host_request_with_interval("two.example", "external_robots_txt", 10_000)
        .unwrap();
    let same_host = first
        .reserve_host_request_with_interval("one.example", "external_page_content", 10_000)
        .unwrap_err();
    assert_eq!(same_host.kind(), std::io::ErrorKind::WouldBlock);
    let global = first
        .reserve_host_request_with_interval("three.example", "serpapi_search", 10_000)
        .unwrap_err();
    assert_eq!(global.kind(), std::io::ErrorKind::WouldBlock);

    first
        .connection
        .execute(
            "UPDATE host_request_log SET lease_until_ms = 0 WHERE id = ?1",
            [a.id],
        )
        .unwrap();
    let recovered = first
        .reserve_host_request_with_interval("one.example", "external_page_content", 10_000)
        .unwrap();
    second.complete_host_request(b.id, Some(200), None).unwrap();
    first
        .complete_host_request(recovered.id, Some(200), None)
        .unwrap();
    drop(first);
    drop(second);
    let _ = std::fs::remove_file(path);
}

#[test]
fn scholar_host_retry_after_and_policy_cache_survive_restarts() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let reservation = store
        .reserve_host_request_with_interval("limited.example", "external_robots_txt", 10_000)
        .unwrap();
    store
        .complete_host_request_with_retry_after(
            reservation.id,
            Some(429),
            Some(120_000),
            Some("rate limited"),
        )
        .unwrap();
    assert_eq!(
        store
            .reserve_host_request_with_interval("limited.example", "external_page_content", 10_000)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    store
        .cache_site_policy(
            "https://limited.example",
            "external_robots_txt",
            200,
            "text/plain",
            "User-agent: *\nDisallow: /private",
            60_000,
        )
        .unwrap();
    drop(store);

    let store = LearningStore::open(&path).unwrap();
    let mut store = store;
    assert_eq!(
        store
            .reserve_host_request_with_interval("limited.example", "external_page_content", 10_000)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock,
        "Retry-After must remain active after a restart"
    );
    let cached = store
        .cached_site_policy("https://limited.example", "external_robots_txt")
        .unwrap()
        .unwrap();
    assert_eq!(cached.status, 200);
    assert_eq!(cached.content_type, "text/plain");
    assert!(cached.body.contains("Disallow: /private"));
    assert!(
        store
            .cache_site_policy(
                "https://limited.example",
                "external_robots_txt",
                302,
                "text/plain",
                "redirect",
                60_000,
            )
            .is_err()
    );
    store
        .connection
        .execute(
            "UPDATE scholar_site_policy_cache SET expires_at_ms = 0 WHERE origin = ?1",
            ["https://limited.example"],
        )
        .unwrap();
    assert!(
        store
            .cached_site_policy("https://limited.example", "external_robots_txt")
            .unwrap()
            .is_none()
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn schema_v37_migrates_persisted_http_safety_state() {
    let path = temp_path();
    let store = LearningStore::open(&path).unwrap();
    store
        .connection
        .execute_batch(
            "DROP TABLE scholar_site_policy_cache;
             DROP TABLE scholar_host_backoff;
             DROP INDEX host_request_log_by_lease;
             ALTER TABLE host_request_log DROP COLUMN lease_until_ms;
             PRAGMA user_version = 37;",
        )
        .unwrap();
    drop(store);

    let mut migrated = LearningStore::open(&path).unwrap();
    let version: i64 = migrated
        .connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, super::super::SCHEMA_VERSION);
    migrated
        .reserve_host_request_with_interval("migration.example", "external_robots_txt", 10_000)
        .unwrap();
    assert!(
        migrated
            .cached_site_policy("https://migration.example", "external_robots_txt")
            .unwrap()
            .is_none()
    );
    drop(migrated);
    let _ = std::fs::remove_file(path);
}
