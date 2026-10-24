use super::*;

impl LearningStore {
    pub fn approve_model_claim(&self, claim_id: i64, reason: &str) -> io::Result<()> {
        require_printable_field(
            reason,
            500,
            "model approval needs a printable reason (1 to 500 characters)",
        )?;
        if let Some(reason) = self.model_claim_training_block(claim_id)? {
            return Err(invalid(reason));
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
        let claim = self.connection
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
                        research_relevance: None,
                    })
                },
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("claim is inactive or has not been approved for model training"))?;
        if let Some(reason) = self.model_claim_training_block(claim_id)? {
            return Err(invalid(reason));
        }
        Ok(claim)
    }

    fn model_claim_training_block(&self, claim_id: i64) -> io::Result<Option<&'static str>> {
        let claim = self
            .connection
            .query_row(
                "SELECT status, retracted_by, volatility, valid_from, valid_until, next_review_at
             FROM semantic_claims WHERE id = ?1",
                [claim_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?;
        let Some((status, retracted_by, volatility, valid_from, valid_until, next_review_at)) =
            claim
        else {
            return Ok(Some("no semantic claim with that ID"));
        };
        if status != "active" || retracted_by.is_some() {
            return Ok(Some(
                "only active, non-withdrawn claims may enter model training",
            ));
        }
        if volatility != "stable" || valid_from.is_some() || valid_until.is_some() {
            return Ok(Some(
                "time-scoped or changing claims must stay in retrieval",
            ));
        }
        let review_due: bool = self
            .connection
            .query_row(
                "SELECT ?1 IS NOT NULL AND ?1 <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
                [next_review_at],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if review_due {
            return Ok(Some("claims due for review must stay in retrieval"));
        }
        let conflict: bool = self
            .connection
            .query_row(
                "SELECT EXISTS (
                SELECT 1 FROM semantic_claims c JOIN semantic_claims other ON other.id != c.id
                WHERE c.id = ?1 AND other.retracted_by IS NULL
                  AND other.status IN ('active', 'uncertain', 'challenged')
                  AND (lower(trim(other.subject)) = lower(trim(c.subject)) OR EXISTS (
                    SELECT 1 FROM entity_equivalences e
                    WHERE lower(trim(e.left_name)) = lower(trim(c.subject))
                      AND lower(trim(e.right_name)) = lower(trim(other.subject))
                  ))
                  AND lower(trim(other.relation)) = lower(trim(c.relation))
                  AND lower(trim(other.object)) != lower(trim(c.object))
             )",
                [claim_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if conflict {
            return Ok(Some(
                "claims with an unresolved competing value must stay in retrieval",
            ));
        }
        let disputed_evidence: bool = self
            .connection
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM claim_evidence_links
                 WHERE claim_id = ?1 AND stance IN ('contradicts', 'qualifies'))",
                [claim_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if disputed_evidence {
            return Ok(Some(
                "claims with contradictory or qualifying evidence must stay in retrieval",
            ));
        }
        let provenance: bool = self
            .connection
            .query_row(
                "SELECT EXISTS (
                SELECT 1 FROM claim_evidence_links l JOIN claim_evidence e ON e.id = l.evidence_id
                WHERE l.claim_id = ?1 AND l.stance = 'supports' AND e.source_kind = 'web'
                  AND (e.source_uri LIKE 'https://%' OR e.source_uri LIKE 'http://%')
                  AND length(trim(e.excerpt)) > 0
             )",
                [claim_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if !provenance {
            return Ok(Some(
                "model training requires linked supporting web evidence with a source URL and excerpt",
            ));
        }
        Ok(None)
    }

    pub fn model_claim_approval_record(&self, claim_id: i64) -> io::Result<(String, String)> {
        self.connection
            .query_row(
                "SELECT reason, approved_at FROM model_claim_approvals WHERE claim_id = ?1",
                [claim_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("claim has no explicit model-training approval"))
    }

    pub fn model_revision_matches(&self, revision: &ConsolidatedClaimRevision) -> io::Result<bool> {
        let Ok(claim) = self.approved_model_claim(revision.id) else {
            return Ok(false);
        };
        let (approval_reason, approved_at) = self.model_claim_approval_record(revision.id)?;
        let evidence = self.claim_evidence(revision.id)?;
        Ok(claim.subject == revision.subject
            && claim.relation == revision.relation
            && claim.object == revision.object
            && claim.source == revision.source
            && claim.valid_from == revision.valid_from
            && claim.valid_until == revision.valid_until
            && approval_reason == revision.approval_reason
            && approved_at == revision.approved_at
            && evidence == revision.evidence)
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
        if !manifest
            .included_claims
            .iter()
            .any(|revision| revision.id == claim_id)
        {
            return Err(invalid("checkpoint manifest omits its consolidated claim"));
        }
        for revision in &manifest.included_claims {
            if !self.model_revision_matches(revision)? {
                return Err(invalid(
                    "checkpoint manifest contains a claim revision that is no longer approved and eligible",
                ));
            }
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
