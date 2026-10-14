//! Storage regressions for training.
use super::*;

#[test]
fn only_active_explicitly_approved_claims_can_train_the_model() {
    let path = temp_path();
    let mut map = KnowledgeMap::default();
    map.add("Auric fern", "has test colour", "violet", "review source")
        .unwrap();
    let mut store = LearningStore::open(&path).unwrap();
    store.import_knowledge(&map).unwrap();
    assert!(store.approved_model_claim(1).is_err());
    assert!(store.approve_model_claim(2, "reviewed").is_err());
    assert!(
        store
            .approve_model_claim(1, "I checked the source")
            .unwrap_err()
            .to_string()
            .contains("supporting web evidence")
    );
    store.connection.execute(
        "INSERT INTO claim_evidence (source_kind, source_uri, excerpt, created_at)
         VALUES ('web', 'https://example.org/fern', 'Auric fern has test colour violet.', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        [],
    ).unwrap();
    store
        .connection
        .execute(
            "INSERT INTO claim_evidence_links (claim_id, evidence_id, stance, linked_at)
         SELECT 1, id, 'supports', strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         FROM claim_evidence WHERE source_uri = 'https://example.org/fern'",
            [],
        )
        .unwrap();
    store
        .approve_model_claim(1, "I checked the source")
        .unwrap();
    assert_eq!(store.approved_model_claim(1).unwrap().object, "violet");
    drop(store);
    let reopened = LearningStore::open(&path).unwrap();
    assert_eq!(
        reopened.approved_model_claim(1).unwrap().subject,
        "Auric fern"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn model_training_blocks_conflicting_or_time_scoped_claims() {
    let path = temp_path();
    let store = LearningStore::open(&path).unwrap();
    store
        .connection
        .execute_batch(
            "INSERT INTO semantic_claims (subject, relation, object, source, created_at)
             VALUES
               ('Mars', 'has moon count', 'two', 'reviewed source', strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
               ('The Red Planet', 'has moon count', 'three', 'reviewed source', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
             INSERT INTO entity_alias_links (left_name, right_name, source_url, evidence_text, source_finding_id, assessment_json)
             VALUES ('Mars', 'The Red Planet', 'https://example.org/alias', 'Mars is also called the Red Planet.', NULL, '{}');
             INSERT INTO claim_evidence (source_kind, source_uri, excerpt, created_at)
             VALUES ('web', 'https://example.org/mars', 'Mars has two moons.', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
             INSERT INTO claim_evidence_links (claim_id, evidence_id, stance, linked_at)
             VALUES (1, 1, 'supports', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));",
        )
        .unwrap();

    assert!(
        store
            .approve_model_claim(1, "reviewed")
            .unwrap_err()
            .to_string()
            .contains("unresolved competing value")
    );
    store
        .connection
        .execute(
            "UPDATE semantic_claims SET status = 'superseded' WHERE id = 2",
            [],
        )
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE semantic_claims SET valid_until = '2026-01-01' WHERE id = 1",
            [],
        )
        .unwrap();
    assert!(
        store
            .approve_model_claim(1, "reviewed")
            .unwrap_err()
            .to_string()
            .contains("time-scoped or changing")
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn model_checkpoint_lineage_roundtrips_exact_claim_revisions() {
    let path = temp_path();
    let mut store = LearningStore::open(&path).unwrap();
    store.connection.execute(
            "INSERT INTO semantic_claims (subject, relation, object, source, created_at)
             VALUES ('Photosynthesis', 'converts', 'light to chemical energy', 'reviewed source', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            [],
        ).unwrap();
    store.connection.execute(
        "INSERT INTO claim_evidence (source_kind, source_uri, excerpt, created_at)
         VALUES ('web', 'https://example.org/photosynthesis', 'Photosynthesis converts light to chemical energy.', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        [],
    ).unwrap();
    store
        .connection
        .execute(
            "INSERT INTO claim_evidence_links (claim_id, evidence_id, stance, linked_at)
         SELECT 1, id, 'supports', strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM claim_evidence",
            [],
        )
        .unwrap();
    store.approve_model_claim(1, "evidence checked").unwrap();
    let (approval_reason, approved_at) = store.model_claim_approval_record(1).unwrap();
    let evidence = store.claim_evidence(1).unwrap();
    let manifest = ModelCheckpointManifest {
        version: 1,
        base_model_dir: "/models/base".into(),
        base_checkpoint: "/models/base/model.safetensors".into(),
        base_checkpoint_sha256: "sha256-base".into(),
        base_adapter_snapshot: None,
        base_adapter_sha256: None,
        included_claims: vec![ConsolidatedClaimRevision {
            id: 1,
            subject: "Photosynthesis".into(),
            relation: "converts".into(),
            object: "light to chemical energy".into(),
            source: "reviewed source".into(),
            valid_from: None,
            valid_until: None,
            approval_reason,
            approved_at,
            evidence,
        }],
        system_prompt: "frozen system prompt".into(),
        max_training_passes: 24,
        loss_threshold: 0.6,
        probe_max_chars: 200,
    };
    let json = serde_json::to_string(&manifest).unwrap();
    let run_id = store
        .record_model_consolidation_with_lineage(
            1,
            "/models/base/model.safetensors",
            "/models/output/model.safetensors",
            0.4,
            "/models/output/consolidation-manifest.json",
            &json,
        )
        .unwrap();
    assert_eq!(store.model_manifest_for_run(run_id).unwrap(), manifest);
    let history = store.model_consolidations().unwrap();
    assert_eq!(history[0].included_claim_ids, [1]);
    assert_eq!(
        history[0].base_checkpoint_sha256.as_deref(),
        Some("sha256-base")
    );
    assert!(store.approved_model_claim(1).is_ok());
    assert!(
        store
            .model_revision_matches(&manifest.included_claims[0])
            .unwrap()
    );
    store
        .connection
        .execute(
            "UPDATE semantic_claims SET object = 'chemical energy from light' WHERE id = 1",
            [],
        )
        .unwrap();
    assert!(
        !store
            .model_revision_matches(&manifest.included_claims[0])
            .unwrap()
    );
    store
        .connection
        .execute(
            "UPDATE semantic_claims SET object = 'light to chemical energy' WHERE id = 1",
            [],
        )
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE claim_evidence SET excerpt = 'Changed source excerpt.' WHERE source_uri = 'https://example.org/photosynthesis'",
            [],
        )
        .unwrap();
    assert!(
        !store
            .model_revision_matches(&manifest.included_claims[0])
            .unwrap()
    );
    store
        .connection
        .execute(
            "UPDATE claim_evidence SET excerpt = 'Photosynthesis converts light to chemical energy.' WHERE source_uri = 'https://example.org/photosynthesis'",
            [],
        )
        .unwrap();
    assert!(
        store
            .model_revision_matches(&manifest.included_claims[0])
            .unwrap()
    );
    store
        .connection
        .execute(
            "UPDATE semantic_claims SET status = 'challenged' WHERE id = 1",
            [],
        )
        .unwrap();
    assert!(store.approved_model_claim(1).is_err());
    assert!(
        !store
            .model_revision_matches(&manifest.included_claims[0])
            .unwrap()
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}
