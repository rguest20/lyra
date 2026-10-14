//! Storage regressions for maintenance.
use super::*;

#[test]
fn maintenance_history_survives_restart_and_marks_stale_runs_interrupted() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let first = store.start_maintenance_pass("chat").unwrap();
    store
        .record_maintenance_role(first, "curator", "idle", 0, None, 1)
        .unwrap();
    store
        .record_maintenance_role(first, "curmudgeon", "error", 0, Some("test failure"), 2)
        .unwrap();
    store.finish_maintenance_pass(first).unwrap();
    let crashed = store.start_maintenance_pass("serve").unwrap();
    store
        .connection
        .execute(
            "UPDATE maintenance_passes SET started_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-11 minutes') WHERE id = ?1",
            [crashed],
        )
        .unwrap();
    drop(store);
    let mut reopened = LearningStore::open(&path).unwrap();
    assert_eq!(
        reopened
            .maintenance_status()
            .unwrap()
            .last_pass
            .unwrap()
            .status,
        "interrupted"
    );
    let next = reopened.start_maintenance_pass("chat").unwrap();
    reopened.finish_maintenance_pass(next).unwrap();
    let history = reopened.maintenance_recent_passes(3).unwrap();
    assert_eq!(
        history
            .iter()
            .map(|run| run.status.as_str())
            .collect::<Vec<_>>(),
        ["completed", "interrupted", "partial_error"]
    );
    assert_eq!(history[2].roles[1].detail.as_deref(), Some("test failure"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn maintenance_pass_history_is_bounded() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    for _ in 0..205 {
        let pass = store.start_maintenance_pass("once").unwrap();
        store.finish_maintenance_pass(pass).unwrap();
    }
    assert_eq!(store.maintenance_recent_passes(300).unwrap().len(), 200);
    let count: i64 = store
        .connection
        .query_row("SELECT COUNT(*) FROM maintenance_passes", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 200);
    let _ = std::fs::remove_file(path);
}

#[test]
fn failed_worker_process_marks_its_running_pass_interrupted() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let pass = store.start_maintenance_pass("serve").unwrap();
    store
        .mark_maintenance_worker_interrupted(std::process::id())
        .unwrap();
    assert_eq!(
        store
            .maintenance_status()
            .unwrap()
            .last_pass
            .unwrap()
            .status,
        "interrupted"
    );
    assert!(store.finish_maintenance_pass(pass).is_err());
    let _ = std::fs::remove_file(path);
}

#[test]
fn maintenance_scan_cursor_covers_claims_in_bounded_persistent_batches() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    for index in 1..=5 {
        store.connection.execute(
                "INSERT INTO semantic_claims (subject, relation, object, source, created_at)
                 VALUES (?1, 'has property', ?2, 'test source', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                rusqlite::params![format!("Topic {index}"), format!("value {index}")],
            ).unwrap();
    }
    let first = store.next_claims_for_maintenance(2).unwrap();
    assert_eq!(
        first.iter().map(|claim| claim.id).collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(
        first
            .iter()
            .all(|claim| claim.relation_count == 1 && claim.evidence_count == 0)
    );
    drop(store);

    let mut store = LearningStore::open(&path).unwrap();
    assert_eq!(
        store
            .next_claims_for_maintenance(2)
            .unwrap()
            .iter()
            .map(|claim| claim.id)
            .collect::<Vec<_>>(),
        [3, 4],
    );
    assert_eq!(store.next_claims_for_maintenance(2).unwrap()[0].id, 5);
    assert_eq!(
        store
            .next_claims_for_maintenance(2)
            .unwrap()
            .iter()
            .map(|claim| claim.id)
            .collect::<Vec<_>>(),
        [1, 2],
        "the cursor wraps after reaching the end so new or revised claims are revisited",
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}
