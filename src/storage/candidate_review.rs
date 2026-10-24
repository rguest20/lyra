use super::*;

struct CandidateReview {
    id: String,
    observation_id: String,
    subject: String,
    relation: String,
    object: String,
    evidence: String,
    context: String,
    resolution: Option<String>,
    confidence: f64,
    status: String,
    valid_from: Option<String>,
    valid_until: Option<String>,
}

impl CandidateReview {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            observation_id: row.get(1)?,
            subject: row.get(2)?,
            relation: row.get(3)?,
            object: row.get(4)?,
            evidence: row.get(5)?,
            context: row.get(6)?,
            resolution: row.get(7)?,
            confidence: row.get(8)?,
            status: row.get(9)?,
            valid_from: row.get(10)?,
            valid_until: row.get(11)?,
        })
    }
}

impl LearningStore {
    pub fn resolve_candidate(&mut self, id: &str, accept: bool, reason: &str) -> io::Result<()> {
        self.resolve_candidate_with_method(id, accept, reason, "human")
    }

    pub fn resolve_candidate_ai(&mut self, id: &str, accept: bool, reason: &str) -> io::Result<()> {
        self.resolve_candidate_with_method(id, accept, reason, "ai")
    }

    fn resolve_candidate_with_method(
        &mut self,
        id: &str,
        accept: bool,
        reason: &str,
        method: &str,
    ) -> io::Result<()> {
        validate_review_reason(reason)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let candidate = tx
            .query_row(
                "SELECT id, observation_id, subject, relation, object, evidence_text,
                    context_message_ids, resolution, confidence, status, valid_from, valid_until
             FROM candidate_claims WHERE id = ?1",
                [id],
                CandidateReview::from_row,
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("no candidate with that ID"))?;
        let CandidateReview {
            id: candidate_id,
            observation_id,
            subject,
            relation,
            object,
            evidence,
            context,
            resolution,
            confidence,
            status,
            valid_from,
            valid_until,
        } = candidate;
        if status != "needs_review" && status != "proposed" {
            return Err(invalid("candidate is already resolved"));
        }
        let observation_text = if accept {
            let observation_text =
                super::observations::read_observation_text(&tx, &observation_id)?;
            if !claim_period_is_stated(
                valid_from.as_deref(),
                valid_until.as_deref(),
                &observation_text,
            ) {
                return Err(invalid(
                    "candidate dates are invalid or not present in the user observation",
                ));
            }
            Some(observation_text)
        } else {
            None
        };
        let semantic_id = if let Some(observation_text) = observation_text.as_deref() {
            let mut claim = CandidateClaim {
                subject: normalize(&subject),
                relation: normalize(&relation),
                object: normalize(&object),
                evidence_text: evidence,
                context_message_ids: serde_json::from_str(&context).map_err(invalid_json)?,
                resolution,
                confidence,
                status: "proposed".into(),
                decision_reason: None,
                valid_from: valid_from.clone(),
                valid_until: valid_until.clone(),
            };
            let semantic_id = match curate_candidate(&tx, &observation_id, &mut claim)? {
                Ok(id) => id,
                Err(conflict) => {
                    let mut statement = tx
                        .prepare(
                            "SELECT id, subject, relation, object, valid_from, valid_until
                         FROM semantic_claims WHERE retracted_by IS NULL",
                        )
                        .map_err(database_error)?;
                    let rows = statement
                        .query_map([], ActiveSemanticClaim::from_row)
                        .map_err(database_error)?;
                    let today = crate::storage::current_sql_date(&tx)?;
                    let aliases = entities::EntityAliases::load(&tx)?;
                    let mut conflicting = None;
                    for row in rows {
                        let existing = row.map_err(database_error)?;
                        if aliases.same(&existing.subject, &claim.subject)
                            && canonical(&existing.relation) == canonical(&claim.relation)
                            && canonical(&existing.object) != canonical(&claim.object)
                            && claim_periods_overlap(
                                claim.valid_from.as_deref(),
                                claim.valid_until.as_deref(),
                                existing.valid_from.as_deref(),
                                existing.valid_until.as_deref(),
                                &today,
                            )
                        {
                            conflicting =
                                Some((existing.id, existing.valid_from, existing.valid_until));
                            break;
                        }
                    }
                    drop(statement);
                    let (current_id, old_from, old_until) =
                        conflicting.ok_or_else(|| invalid(&conflict))?;
                    let supersedes_id = if old_from.as_deref() == claim.valid_from.as_deref()
                        && old_until.as_deref() == claim.valid_until.as_deref()
                    {
                        research::supersede_semantic_claim(
                            &tx,
                            current_id,
                            &format!(
                                "superseded by reviewed candidate {candidate_id}: {}",
                                reason.trim()
                            ),
                        )?;
                        Some(current_id)
                    } else {
                        research::mark_semantic_claim_challenged(&tx, current_id)?;
                        None
                    };
                    tx.execute(
                        "INSERT INTO semantic_claims
                            (subject, relation, object, source, supersedes_id, source_observation_id, created_at, observed_at, valid_from, valid_until)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                             (SELECT created_at FROM observations WHERE id = ?6), ?7, ?8)",
                        params![claim.subject, claim.relation, claim.object, format!("reviewed candidate {candidate_id}: {}", reason.trim()), supersedes_id, observation_id, claim.valid_from, claim.valid_until],
                    ).map_err(database_error)?;
                    tx.last_insert_rowid()
                }
            };
            link_candidate_to_semantic_claim(&tx, semantic_id, &candidate_id, &observation_id)?;
            link_claim_evidence(
                &tx,
                semantic_id,
                "user_observation",
                None,
                Some(&observation_id),
                observation_text,
                "supports",
            )?;
            Some(semantic_id)
        } else {
            None
        };
        tx.execute(
            "UPDATE candidate_claims SET status = ?2, decision_reason = ?3, semantic_claim_id = ?4 WHERE id = ?1",
            params![candidate_id, if accept { "promoted" } else { "rejected" }, reason.trim(), semantic_id],
        ).map_err(database_error)?;
        let proposal_id: i64 = tx.query_row(
            "SELECT id FROM role_proposals WHERE source_type = 'candidate_claim' AND source_id = ?1 ORDER BY id DESC LIMIT 1",
            [&candidate_id], |row| row.get(0),
        ).map_err(database_error)?;
        record_policy_decision(
            &tx,
            proposal_id,
            review_decision_outcome(method, accept),
            reason.trim(),
            semantic_id,
            Some(if accept { "active" } else { "rejected" }),
        )?;
        let remaining: i64 = tx.query_row(
            "SELECT COUNT(*) FROM candidate_claims WHERE observation_id = ?1 AND status IN ('proposed', 'needs_review')",
            [&observation_id], |row| row.get(0),
        ).map_err(database_error)?;
        if remaining == 0 {
            let promoted: i64 = tx.query_row(
                "SELECT COUNT(*) FROM candidate_claims WHERE observation_id = ?1 AND status = 'promoted'",
                [&observation_id], |row| row.get(0),
            ).map_err(database_error)?;
            tx.execute(
                "UPDATE observations SET status = ?2, last_error = NULL WHERE id = ?1",
                params![
                    observation_id,
                    if promoted > 0 { "promoted" } else { "rejected" }
                ],
            )
            .map_err(database_error)?;
        }
        tx.commit().map_err(database_error)
    }
}
