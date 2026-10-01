use super::*;

impl LearningStore {
    pub fn approve_model_claim(&self, claim_id: i64, reason: &str) -> io::Result<()> {
        if reason.trim().is_empty()
            || reason.chars().count() > 500
            || reason.chars().any(char::is_control)
        {
            return Err(invalid(
                "model approval needs a printable reason (1 to 500 characters)",
            ));
        }
        let changed = self.connection.execute(
            "INSERT INTO model_claim_approvals (claim_id, reason, approved_at)
             SELECT id, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             FROM semantic_claims WHERE id = ?1 AND retracted_by IS NULL AND status = 'active'
             ON CONFLICT(claim_id) DO UPDATE SET reason = excluded.reason, approved_at = excluded.approved_at",
            params![claim_id, reason.trim()],
        ).map_err(database_error)?;
        if changed == 0 {
            return Err(invalid("no active semantic claim with that ID"));
        }
        Ok(())
    }

    pub fn approved_model_claim(&self, claim_id: i64) -> io::Result<SemanticClaim> {
        self.connection
            .query_row(
                "SELECT c.id, c.subject, c.relation, c.object, c.source, c.source_observation_id,
                    c.status, c.volatility, c.valid_from, c.valid_until, c.observed_at, c.checked_at,
                    c.last_reviewed_at, c.next_review_at
             FROM semantic_claims c JOIN model_claim_approvals a ON a.claim_id = c.id
             WHERE c.id = ?1 AND c.retracted_by IS NULL AND c.status = 'active'",
                [claim_id],
                |row| {
                    Ok(SemanticClaim {
                        id: row.get(0)?,
                        subject: row.get(1)?,
                        relation: row.get(2)?,
                        object: row.get(3)?,
                        source: row.get(4)?,
                        source_observation_id: row.get(5)?,
                        status: row.get(6)?, volatility: row.get(7)?, valid_from: row.get(8)?,
                        valid_until: row.get(9)?, observed_at: row.get(10)?, checked_at: row.get(11)?,
                        last_reviewed_at: row.get(12)?, next_review_at: row.get(13)?,
                        extraction_confidence: None, source_quality: None, freshness: None,
                    })
                },
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("claim is inactive or has not been approved for model training"))
    }

    pub fn record_model_consolidation_with_lineage(
        &mut self,
        claim_id: i64,
        source: &str,
        output: &str,
        loss: f32,
        manifest_path: &str,
        manifest_json: &str,
    ) -> io::Result<i64> {
        let manifest: ModelCheckpointManifest =
            serde_json::from_str(manifest_json).map_err(invalid_json)?;
        if manifest.version != 1 || manifest.included_claims.is_empty() {
            return Err(invalid(
                "checkpoint manifest is invalid or has no included claims",
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        tx.execute(
            "INSERT INTO model_consolidations (claim_id, source_checkpoint, output_checkpoint, training_loss, created_at)
             VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![claim_id, source, output, loss],
        ).map_err(database_error)?;
        let run_id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO model_checkpoint_lineage (output_checkpoint, manifest_path, manifest_json, created_at)
             VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![output, manifest_path, manifest_json],
        ).map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        Ok(run_id)
    }

    pub fn model_manifest_for_run(&self, run_id: i64) -> io::Result<ModelCheckpointManifest> {
        let json: String = self
            .connection
            .query_row(
                "SELECT l.manifest_json FROM model_consolidations c
             JOIN model_checkpoint_lineage l ON l.output_checkpoint = c.output_checkpoint
             WHERE c.id = ?1",
                [run_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("consolidation has no lineage manifest (legacy run)"))?;
        serde_json::from_str(&json).map_err(invalid_json)
    }

    pub fn model_manifest_for_checkpoint(
        &self,
        output_checkpoint: &str,
    ) -> io::Result<Option<ModelCheckpointManifest>> {
        let json: Option<String> = self
            .connection
            .query_row(
                "SELECT manifest_json FROM model_checkpoint_lineage WHERE output_checkpoint = ?1",
                [output_checkpoint],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)?;
        json.map(|value| serde_json::from_str(&value).map_err(invalid_json))
            .transpose()
    }

    pub fn claim_is_active_and_approved(&self, claim_id: i64) -> io::Result<bool> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM semantic_claims c
             JOIN model_claim_approvals a ON a.claim_id = c.id
             WHERE c.id = ?1 AND c.status = 'active' AND c.retracted_by IS NULL)",
                [claim_id],
                |row| row.get(0),
            )
            .map_err(database_error)
    }

    pub fn model_consolidations(&self) -> io::Result<Vec<ModelConsolidation>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT c.id, c.claim_id, c.source_checkpoint, c.output_checkpoint, c.training_loss,
                    l.manifest_path, l.manifest_json
             FROM model_consolidations c LEFT JOIN model_checkpoint_lineage l
               ON l.output_checkpoint = c.output_checkpoint
             ORDER BY c.id DESC LIMIT 30",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, f32>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        rows.into_iter()
            .map(
                |(
                    id,
                    claim_id,
                    source_checkpoint,
                    output_checkpoint,
                    training_loss,
                    manifest_path,
                    json,
                )| {
                    let manifest = json
                        .map(|value| {
                            serde_json::from_str::<ModelCheckpointManifest>(&value)
                                .map_err(invalid_json)
                        })
                        .transpose()?;
                    Ok(ModelConsolidation {
                        id,
                        claim_id,
                        source_checkpoint,
                        output_checkpoint,
                        training_loss,
                        manifest_path,
                        base_checkpoint_sha256: manifest
                            .as_ref()
                            .map(|value| value.base_checkpoint_sha256.clone()),
                        included_claim_ids: manifest
                            .map(|value| {
                                value
                                    .included_claims
                                    .into_iter()
                                    .map(|claim| claim.id)
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                },
            )
            .collect()
    }
}
