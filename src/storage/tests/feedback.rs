//! Storage regressions for feedback.
use super::*;

#[test]
fn feedback_budget_is_three_total_across_routes_and_restarts() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let (original_job, root) = feedback_fixture(&mut store);
    store
        .reject_scholar_finding_with_feedback(
            root,
            "Needs better extraction",
            "repair",
            "Preserve the source's actual wording.",
        )
        .unwrap();
    drop(store);
    let mut store = LearningStore::open(&path).unwrap();
    let repair = store.claim_scholar_feedback("repair").unwrap().unwrap();
    assert_eq!((repair.root_finding_id, repair.attempt), (root, 2));
    assert_eq!(repair.question, "What kind of body is Mars?");
    assert_eq!(repair.source_excerpt, "Mars is a planet.");
    assert_eq!(repair.guidance, "Preserve the source's actual wording.");
    let child = feedback_finding(&mut store, &repair.job_id);
    store
        .finish_scholar_feedback(&repair, true, "Re-extracted for review")
        .unwrap();
    assert_eq!(
        store.scholar_findings(&repair.job_id).unwrap()[0].status,
        "needs_review"
    );
    assert!(store.active_semantic_claims().unwrap().is_empty());
    store
        .reject_scholar_finding_with_source_feedback(
            child,
            "Unsuitable source",
            "new_source",
            "Find an independent astronomy reference.",
            Some("irrelevant"),
            None,
        )
        .unwrap();
    let search = store.claim_scholar_feedback("new_source").unwrap().unwrap();
    assert_eq!((search.root_finding_id, search.attempt), (root, 3));
    assert_eq!(search.question, repair.question);
    assert_eq!(
        store
            .scholar_feedback_excluded_sources(&search.job_id)
            .unwrap(),
        vec!["https://evidence.example/mars"]
    );
    let job = store.claim_scholar_job().unwrap().unwrap();
    assert_eq!(job.id, search.job_id);
    let last = feedback_finding(&mut store, &job.id);
    store
        .finish_scholar_feedback(&search, true, "Another candidate")
        .unwrap();
    store
        .reject_scholar_finding_with_feedback(
            last,
            "Still unsupported",
            "repair",
            "Read the exact passage.",
        )
        .unwrap();
    assert!(store.claim_scholar_feedback("repair").unwrap().is_none());
    assert!(
        store
            .claim_scholar_feedback("new_source")
            .unwrap()
            .is_none()
    );
    let session: (i64, String) = store
        .connection
        .query_row(
            "SELECT attempts, status FROM scholar_feedback_sessions WHERE root_finding_id = ?1",
            [root],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(session, (3, "exhausted".into()));
    assert_eq!(
        store.scholar_findings(&original_job).unwrap()[0].status,
        "rejected"
    );
    assert_eq!(store.scholar_jobs().unwrap().len(), 3);
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn feedback_failed_attempt_recovery_is_idempotent_and_bounded() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let (_, root) = feedback_fixture(&mut store);
    store
        .reject_scholar_finding_with_feedback(
            root,
            "Bad extraction",
            "repair",
            "Try the exact passage again.",
        )
        .unwrap();
    let repair = store.claim_scholar_feedback("repair").unwrap().unwrap();
    // A stale lease resumes the same attempt and job, without spending another go.
    store.connection.execute("UPDATE scholar_research_feedback SET claimed_at = '2000-01-01T00:00:00Z' WHERE id = ?1", [repair.id]).unwrap();
    let reclaimed = store.claim_scholar_feedback("repair").unwrap().unwrap();
    assert_eq!(
        (reclaimed.id, reclaimed.job_id.as_str(), reclaimed.attempt),
        (repair.id, repair.job_id.as_str(), 2)
    );
    store
        .finish_scholar_job(&repair.job_id, "", "", "", None, Some("No grounded claim"))
        .unwrap();
    // Simulate interruption after the job completes but before feedback completion.
    drop(store);
    let mut store = LearningStore::open(&path).unwrap();
    store.recover_finished_scholar_feedback().unwrap();
    store.recover_finished_scholar_feedback().unwrap();
    store
        .finish_scholar_feedback(&repair, false, "Repeated completion")
        .unwrap();
    let search = store.claim_scholar_feedback("new_source").unwrap().unwrap();
    assert_eq!(search.attempt, 3);
    assert!(
        store
            .scholar_feedback_excluded_sources(&search.job_id)
            .unwrap()
            .is_empty()
    );
    assert!(
        search
            .guidance
            .contains("Previous attempt produced no grounded finding")
    );
    let job = store.claim_scholar_job().unwrap().unwrap();
    store
        .finish_scholar_job(&job.id, "", "", "", None, Some("No suitable source"))
        .unwrap();
    store
        .finish_scholar_feedback(&search, false, "No suitable source")
        .unwrap();
    store
        .finish_scholar_feedback(&search, false, "Duplicate completion")
        .unwrap();
    assert!(
        store
            .claim_scholar_feedback("new_source")
            .unwrap()
            .is_none()
    );
    assert_eq!(store.scholar_jobs().unwrap().len(), 3);
    assert_eq!(
        store
            .connection
            .query_row("SELECT status FROM scholar_feedback_sessions", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "exhausted"
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn invalid_feedback_rolls_back_rejection_and_acceptance_stops_retrying() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let (job, root) = feedback_fixture(&mut store);
    assert!(
        store
            .reject_scholar_finding_with_feedback(root, "Fixable", "repair", "")
            .is_err()
    );
    assert_eq!(
        store.scholar_findings(&job).unwrap()[0].status,
        "needs_review"
    );
    assert!(store.claim_scholar_feedback("repair").unwrap().is_none());
    store
        .reject_scholar_finding_with_feedback(root, "Fixable", "repair", "Re-extract faithfully.")
        .unwrap();
    let task = store.claim_scholar_feedback("repair").unwrap().unwrap();
    let child = feedback_finding(&mut store, &task.job_id);
    store
        .finish_scholar_feedback(&task, true, "Extracted")
        .unwrap();
    store
        .review_scholar_finding_ai(child, true, "Exact passage supports it.")
        .unwrap();
    assert_eq!(
        store
            .connection
            .query_row("SELECT status FROM scholar_feedback_sessions", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "accepted"
    );
    assert!(store.claim_scholar_feedback("repair").unwrap().is_none());
    assert_eq!(store.active_semantic_claims().unwrap().len(), 1);
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn repeated_repair_of_the_same_proposition_and_passage_records_no_progress() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let (_, root) = feedback_fixture(&mut store);
    store
        .reject_scholar_finding_with_feedback(
            root,
            "The candidate still has the same unsupported scope.",
            "repair",
            "Narrow the claim to what the passage establishes.",
        )
        .unwrap();
    let task = store.claim_scholar_feedback("repair").unwrap().unwrap();
    let repeated = feedback_finding(&mut store, &task.job_id);
    store
        .finish_scholar_feedback(&task, true, "A finding was proposed for review")
        .unwrap();
    store
        .reject_scholar_finding_with_feedback(
            repeated,
            "The candidate still has the same unsupported scope.",
            "repair",
            "Narrow the claim to what the passage establishes.",
        )
        .unwrap();
    assert!(store.claim_scholar_feedback("repair").unwrap().is_none());
    let (attempts, status, reason): (i64, String, String) = store
        .connection
        .query_row(
            "SELECT attempts,status,last_reason FROM scholar_feedback_sessions WHERE root_finding_id=?1",
            [root],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        )
        .unwrap();
    assert_eq!(attempts, 2);
    assert_eq!(status, "exhausted");
    assert!(reason.starts_with("No progress:"));
    assert_eq!(
        store.scholar_findings(&task.job_id).unwrap()[0].status,
        "rejected"
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn feedback_migration_preserves_existing_findings() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let (job, root) = feedback_fixture(&mut store);
    store.connection.execute_batch("DROP TABLE scholar_research_feedback; DROP TABLE scholar_feedback_sessions; PRAGMA user_version = 39;").unwrap();
    drop(store);
    let mut store = LearningStore::open(&path).unwrap();
    assert_eq!(store.scholar_findings(&job).unwrap()[0].id, root);
    store
        .reject_scholar_finding_with_feedback(root, "Fixable", "repair", "Read the passage.")
        .unwrap();
    assert_eq!(
        store
            .claim_scholar_feedback("repair")
            .unwrap()
            .unwrap()
            .attempt,
        2
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn source_feedback_preserves_suggested_evidence_and_does_not_exclude_on_failure() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let (job, root) = feedback_fixture(&mut store);
    assert!(
        store
            .reject_scholar_finding_with_source_feedback(
                root,
                "Wrong wording",
                "repair",
                "Extract the classification.",
                None,
                Some("Mars is a moon.")
            )
            .is_err()
    );
    assert_eq!(
        store.scholar_findings(&job).unwrap()[0].status,
        "needs_review"
    );
    assert!(
        store
            .reject_scholar_finding_with_source_feedback(
                root,
                "Wrong wording",
                "repair",
                "Extract the classification.",
                Some("unreliable"),
                None
            )
            .is_err()
    );
    store
        .reject_scholar_finding_with_source_feedback(
            root,
            "Wrong wording",
            "repair",
            "Extract the classification.",
            None,
            Some("Mars is a planet."),
        )
        .unwrap();
    drop(store);
    let mut store = LearningStore::open(&path).unwrap();
    let task = store.claim_scholar_feedback("repair").unwrap().unwrap();
    assert_eq!(
        task.suggested_evidence.as_deref(),
        Some("Mars is a planet.")
    );
    assert!(task.exclusion_reason.is_none());
    store
        .finish_scholar_job(&task.job_id, "", "", "", None, Some("Extraction failed"))
        .unwrap();
    store
        .finish_scholar_feedback(&task, false, "Extraction failed")
        .unwrap();
    let search = store.claim_scholar_feedback("new_source").unwrap().unwrap();
    assert_eq!(search.attempt, 3);
    assert!(search.exclusion_reason.is_none());
    assert!(
        store
            .scholar_feedback_excluded_sources(&search.job_id)
            .unwrap()
            .is_empty()
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn version_40_feedback_migration_retains_budget_without_ambiguous_exclusions() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let (_, root) = feedback_fixture(&mut store);
    store
        .reject_scholar_finding_with_feedback(
            root,
            "Old ambiguous review",
            "new_source",
            "Find something more relevant.",
        )
        .unwrap();
    store.connection.execute_batch("ALTER TABLE scholar_research_feedback DROP COLUMN exclusion_reason; ALTER TABLE scholar_research_feedback DROP COLUMN suggested_evidence; PRAGMA user_version = 40;").unwrap();
    drop(store);
    let mut store = LearningStore::open(&path).unwrap();
    let task = store.claim_scholar_feedback("new_source").unwrap().unwrap();
    assert_eq!(task.attempt, 2);
    assert_eq!(task.root_finding_id, root);
    assert_eq!(task.guidance, "Find something more relevant.");
    assert!(task.exclusion_reason.is_none());
    assert!(
        store
            .scholar_feedback_excluded_sources(&task.job_id)
            .unwrap()
            .is_empty()
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn source_exclusion_is_confined_to_one_finding_retry_chain_after_restart() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    let (original_job, root) = feedback_fixture(&mut store);
    store
        .reject_scholar_finding_with_source_feedback(
            root,
            "Irrelevant to this claim",
            "new_source",
            "Find evidence for the original question.",
            Some("irrelevant"),
            None,
        )
        .unwrap();
    let retry = store.claim_scholar_feedback("new_source").unwrap().unwrap();
    let excluded = "https://evidence.example/mars";
    assert_eq!(
        store
            .scholar_feedback_excluded_sources(&retry.job_id)
            .unwrap(),
        vec![excluded]
    );
    assert!(
        store
            .scholar_feedback_excluded_sources(&original_job)
            .unwrap()
            .is_empty()
    );
    // Finish the retry job, then research the identical question independently.
    assert_eq!(store.claim_scholar_job().unwrap().unwrap().id, retry.job_id);
    store
        .finish_scholar_job(&retry.job_id, "", "", "", None, Some("No result"))
        .unwrap();
    store
        .finish_scholar_feedback(&retry, true, "Completed test retry")
        .unwrap();
    store
        .enqueue_research("independent-claim", 1, "What kind of body is Mars?")
        .unwrap();
    let independent = store.claim_scholar_job().unwrap().unwrap();
    assert!(
        store
            .scholar_feedback_excluded_sources(&independent.id)
            .unwrap()
            .is_empty()
    );
    let independent_finding = feedback_finding(&mut store, &independent.id);
    store
        .reject_scholar_finding_with_feedback(
            independent_finding,
            "Improve wording",
            "repair",
            "Keep the supported detail.",
        )
        .unwrap();
    let independent_retry = store.claim_scholar_feedback("repair").unwrap().unwrap();
    assert_ne!(independent_retry.root_finding_id, root);
    assert!(
        store
            .scholar_feedback_excluded_sources(&independent_retry.job_id)
            .unwrap()
            .is_empty()
    );
    // The page is also available to an entirely different research question.
    store
        .enqueue_research(
            "different-claim",
            1,
            "What is the history of Mars observation?",
        )
        .unwrap();
    let different = store.claim_scholar_job().unwrap().unwrap();
    assert!(
        store
            .scholar_feedback_excluded_sources(&different.id)
            .unwrap()
            .is_empty()
    );
    drop(store);
    let store = LearningStore::open(&path).unwrap();
    assert_eq!(
        store
            .scholar_feedback_excluded_sources(&retry.job_id)
            .unwrap(),
        vec![excluded]
    );
    for job in [
        &original_job,
        &independent.id,
        &independent_retry.job_id,
        &different.id,
    ] {
        assert!(
            store
                .scholar_feedback_excluded_sources(job)
                .unwrap()
                .is_empty()
        );
    }
    drop(store);
    let _ = std::fs::remove_file(path);
}
