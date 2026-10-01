use super::*;

impl LearningStore {
    pub fn resolve_candidate(&mut self, id: &str, accept: bool, reason: &str) -> io::Result<()> {
        if reason.trim().is_empty()
            || reason.chars().count() > 500
            || reason.chars().any(char::is_control)
        {
            return Err(invalid(
                "review reason must contain 1 to 500 printable characters",
            ));
        }
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
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, f64>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, Option<String>>(10)?,
                        row.get::<_, Option<String>>(11)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("no candidate with that ID"))?;
        let (
            candidate_id,
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
        ) = candidate;
        if status != "needs_review" && status != "proposed" {
            return Err(invalid("candidate is already resolved"));
        }
        if accept {
            let observation_text: String = tx
                .query_row(
                    "SELECT raw_text FROM observations WHERE id = ?1",
                    [&observation_id],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            if !valid_claim_period(valid_from.as_deref(), valid_until.as_deref())
                || [valid_from.as_deref(), valid_until.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|date| !observation_text.contains(date))
            {
                return Err(invalid(
                    "candidate dates are invalid or not present in the user observation",
                ));
            }
        }
        let semantic_id = if accept {
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
                        .query_map([], |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, String>(3)?,
                                row.get::<_, Option<String>>(4)?,
                                row.get::<_, Option<String>>(5)?,
                            ))
                        })
                        .map_err(database_error)?;
                    let today: String = tx
                        .query_row("SELECT date('now')", [], |row| row.get(0))
                        .map_err(database_error)?;
                    let mut conflicting = None;
                    for row in rows {
                        let (id, subject, relation, object, from, until) =
                            row.map_err(database_error)?;
                        if canonical(&subject) == canonical(&claim.subject)
                            && canonical(&relation) == canonical(&claim.relation)
                            && canonical(&object) != canonical(&claim.object)
                            && claim_periods_overlap(
                                claim.valid_from.as_deref(),
                                claim.valid_until.as_deref(),
                                from.as_deref(),
                                until.as_deref(),
                                &today,
                            )
                        {
                            conflicting = Some((id, from, until));
                            break;
                        }
                    }
                    drop(statement);
                    let (current_id, old_from, old_until) =
                        conflicting.ok_or_else(|| invalid(&conflict))?;
                    let supersedes_id = if old_from.as_deref() == claim.valid_from.as_deref()
                        && old_until.as_deref() == claim.valid_until.as_deref()
                    {
                        tx.execute(
                            "UPDATE semantic_claims SET retracted_by = ?2, status = 'superseded' WHERE id = ?1",
                            params![current_id, format!("superseded by reviewed candidate {candidate_id}: {}", reason.trim())],
                        ).map_err(database_error)?;
                        Some(current_id)
                    } else {
                        tx.execute(
                            "UPDATE semantic_claims SET status = 'challenged' WHERE id = ?1",
                            [current_id],
                        )
                        .map_err(database_error)?;
                        None
                    };
                    tx.execute(
                        "INSERT INTO semantic_claims
                            (subject, relation, object, source, supersedes_id, source_observation_id, created_at, observed_at, valid_from, valid_until)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                             (SELECT created_at FROM observations WHERE id = ?6), ?7, ?8)",
                        params![claim.subject, claim.relation, claim.object, format!("reviewed candidate {candidate_id}: {}", reason.trim()), supersedes_id, observation_id, claim.valid_from, claim.valid_until],
                    ).map_err(database_error)?;
                    tx.query_row("SELECT last_insert_rowid()", [], |row| row.get(0))
                        .map_err(database_error)?
                }
            };
            tx.execute("INSERT OR IGNORE INTO semantic_claim_sources (semantic_claim_id, candidate_id, observation_id) VALUES (?1, ?2, ?3)", params![semantic_id, candidate_id, observation_id]).map_err(database_error)?;
            let observation_text: String = tx
                .query_row(
                    "SELECT raw_text FROM observations WHERE id = ?1",
                    [&observation_id],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            link_claim_evidence(
                &tx,
                semantic_id,
                "user_observation",
                None,
                Some(&observation_id),
                &observation_text,
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
            if accept {
                "human_accepted"
            } else {
                "human_rejected"
            },
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
