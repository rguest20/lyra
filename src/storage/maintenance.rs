use super::*;

impl LearningStore {
    pub fn claim_evidence(&self, claim_id: i64) -> io::Result<Vec<ClaimEvidence>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT e.id, e.source_kind, e.source_uri, e.observation_id, e.excerpt,
                    l.stance, e.published_at, e.retrieved_at
             FROM claim_evidence_links l JOIN claim_evidence e ON e.id = l.evidence_id
             WHERE l.claim_id = ?1 ORDER BY e.id",
            )
            .map_err(database_error)?;
        statement
            .query_map([claim_id], |row| {
                Ok(ClaimEvidence {
                    id: row.get(0)?,
                    source_kind: row.get(1)?,
                    source_uri: row.get(2)?,
                    observation_id: row.get(3)?,
                    excerpt: row.get(4)?,
                    stance: row.get(5)?,
                    published_at: row.get(6)?,
                    retrieved_at: row.get(7)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn due_claim_ids(&self, limit: usize) -> io::Result<Vec<i64>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT id FROM semantic_claims
                 WHERE retracted_by IS NULL AND status IN ('active', 'uncertain', 'challenged')
                   AND next_review_at IS NOT NULL
                   AND next_review_at <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 ORDER BY next_review_at, id LIMIT ?1",
            )
            .map_err(database_error)?;
        statement
            .query_map(
                [i64::try_from(limit).map_err(|_| invalid("limit too large"))?],
                |row| row.get(0),
            )
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    /// Return and durably advance over a bounded rotating slice of live claims.
    /// Challenge and due queues are handled separately before Scout uses this
    /// background pass for lower-priority gaps.
    pub fn next_claims_for_maintenance(
        &mut self,
        limit: usize,
    ) -> io::Result<Vec<MaintenanceReviewClaim>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit = limit.min(100);
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let cursor: i64 = tx
            .query_row(
                "SELECT last_claim_id FROM maintenance_scan_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)?
            .unwrap_or(0);
        let fetch = |after: i64| -> io::Result<Vec<MaintenanceReviewClaim>> {
            let mut statement = tx
                .prepare(
                    "SELECT c.id, c.subject, c.relation, c.object,
                        (SELECT COUNT(DISTINCT lower(trim(r.relation))) FROM semantic_claims r
                         WHERE r.retracted_by IS NULL AND r.status IN ('active', 'uncertain', 'challenged')
                           AND lower(trim(r.subject)) = lower(trim(c.subject))),
                        (SELECT COUNT(*) FROM claim_evidence_links e WHERE e.claim_id = c.id)
                     FROM semantic_claims c
                     WHERE c.retracted_by IS NULL AND c.status IN ('active', 'uncertain', 'challenged')
                       AND c.id > ?1
                     ORDER BY c.id LIMIT ?2",
                )
                .map_err(database_error)?;
            statement
                .query_map(params![after, i64::try_from(limit).unwrap_or(100)], |row| {
                    let relations: i64 = row.get(4)?;
                    let evidence: i64 = row.get(5)?;
                    Ok(MaintenanceReviewClaim {
                        id: row.get(0)?,
                        subject: row.get(1)?,
                        relation: row.get(2)?,
                        object: row.get(3)?,
                        relation_count: usize::try_from(relations).unwrap_or(usize::MAX),
                        evidence_count: usize::try_from(evidence).unwrap_or(usize::MAX),
                    })
                })
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)
        };
        let mut claims = fetch(cursor)?;
        if claims.is_empty() {
            claims = fetch(0)?;
        }
        if let Some(last) = claims.last() {
            tx.execute(
                "INSERT INTO maintenance_scan_state (id, last_claim_id, updated_at)
                 VALUES (1, ?1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                 ON CONFLICT(id) DO UPDATE SET last_claim_id = excluded.last_claim_id,
                    updated_at = excluded.updated_at",
                [last.id],
            )
            .map_err(database_error)?;
        }
        tx.commit().map_err(database_error)?;
        Ok(claims)
    }

    pub fn due_claim_topics(&self, limit: usize) -> io::Result<Vec<(i64, String, String, String)>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, subject, relation, object FROM semantic_claims
             WHERE retracted_by IS NULL AND status IN ('active', 'uncertain', 'challenged')
               AND next_review_at IS NOT NULL
               AND next_review_at <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             ORDER BY next_review_at, id LIMIT ?1",
            )
            .map_err(database_error)?;
        statement
            .query_map(
                [i64::try_from(limit).map_err(|_| invalid("limit too large"))?],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn challenged_claim_topics(
        &self,
        limit: usize,
    ) -> io::Result<Vec<(i64, String, String, String)>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, subject, relation, object FROM semantic_claims
             WHERE retracted_by IS NULL AND status = 'challenged'
             ORDER BY id LIMIT ?1",
            )
            .map_err(database_error)?;
        statement
            .query_map(
                [i64::try_from(limit).map_err(|_| invalid("limit too large"))?],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    /// Record an explicit policy decision after a maintenance review.
    pub fn record_claim_review(
        &self,
        claim_id: i64,
        status: &str,
        checked_at: Option<&str>,
        next_review_at: Option<&str>,
    ) -> io::Result<()> {
        if !matches!(status, "active" | "uncertain" | "challenged") {
            return Err(invalid("invalid review state"));
        }
        for value in [checked_at, next_review_at].into_iter().flatten() {
            let valid: bool = self
                .connection
                .query_row("SELECT datetime(?1) IS NOT NULL", [value], |row| row.get(0))
                .map_err(database_error)?;
            if !valid {
                return Err(invalid("review dates must be valid ISO date-times"));
            }
        }
        let changed = self
            .connection
            .execute(
                "UPDATE semantic_claims SET status = ?2,
                 checked_at = COALESCE(?3, checked_at),
                 last_reviewed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 next_review_at = ?4
             WHERE id = ?1 AND retracted_by IS NULL",
                params![claim_id, status, checked_at, next_review_at],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("claim is not active"));
        }
        Ok(())
    }

    pub fn resolve_semantic_claim(
        &mut self,
        claim_id: i64,
        action: &str,
        reason: &str,
    ) -> io::Result<()> {
        if !matches!(action, "confirm" | "challenge" | "qualify" | "retract") {
            return Err(invalid(
                "action must be confirm, challenge, qualify, or retract",
            ));
        }
        validate_review_reason(reason)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM semantic_claims WHERE id = ?1 AND retracted_by IS NULL)",
                [claim_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if !exists {
            return Err(invalid("claim is not currently active"));
        }
        let (status, stance, outcome) = match action {
            "confirm" => ("active", "supports", "human_confirmed"),
            "challenge" => ("challenged", "contradicts", "human_challenged"),
            "qualify" => ("uncertain", "qualifies", "human_qualified"),
            "retract" => ("retracted", "contradicts", "human_retracted"),
            _ => unreachable!(),
        };
        tx.execute(
            "UPDATE semantic_claims SET status = ?2,
                retracted_by = CASE WHEN ?2 = 'retracted' THEN ?3 ELSE retracted_by END,
                checked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                last_reviewed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                next_review_at = NULL
             WHERE id = ?1 AND retracted_by IS NULL",
            params![claim_id, status, reason.trim()],
        )
        .map_err(database_error)?;
        link_claim_evidence(&tx, claim_id, "manual", None, None, reason.trim(), stance)?;
        let proposal_id = record_role_proposal(
            &tx,
            RoleProposal {
                role: "human_reviewer",
                source_type: "semantic_claim",
                source_id: &claim_id.to_string(),
                suggested_action: action,
                target_claim_id: Some(claim_id),
                evidence_refs: "[\"manual review\"]",
                payload: &serde_json::to_string(&serde_json::json!({ "reason": reason.trim() }))
                    .map_err(invalid_json)?,
            },
        )?;
        record_policy_decision(
            &tx,
            proposal_id,
            outcome,
            reason.trim(),
            Some(claim_id),
            Some(status),
        )?;
        tx.commit().map_err(database_error)
    }

    pub fn supersede_semantic_claim(
        &mut self,
        old_claim_id: i64,
        new_claim_id: i64,
        reason: &str,
    ) -> io::Result<()> {
        if old_claim_id == new_claim_id {
            return Err(invalid("a claim cannot supersede itself"));
        }
        validate_review_reason(reason)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let old = tx
            .query_row(
                "SELECT subject, relation, object FROM semantic_claims WHERE id = ?1 AND retracted_by IS NULL",
                [old_claim_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("old claim is not currently active"))?;
        let new = tx
            .query_row(
                "SELECT subject, relation, object FROM semantic_claims WHERE id = ?1 AND retracted_by IS NULL",
                [new_claim_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("replacement claim is not currently active"))?;
        if canonical(&old.0) != canonical(&new.0)
            || canonical(&old.1) != canonical(&new.1)
            || canonical(&old.2) == canonical(&new.2)
        {
            return Err(invalid(
                "replacement must change the object of the same subject and relation",
            ));
        }
        tx.execute(
            "UPDATE semantic_claims SET status = 'superseded', retracted_by = ?2,
                checked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                last_reviewed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), next_review_at = NULL
             WHERE id = ?1",
            params![
                old_claim_id,
                format!("superseded by claim #{new_claim_id}: {}", reason.trim())
            ],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE semantic_claims SET supersedes_id = ?2,
                source = source || '; supersedes claim #' || ?2,
                checked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                last_reviewed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            params![new_claim_id, old_claim_id],
        )
        .map_err(database_error)?;
        link_claim_evidence(
            &tx,
            old_claim_id,
            "manual",
            None,
            None,
            reason.trim(),
            "contradicts",
        )?;
        link_claim_evidence(
            &tx,
            new_claim_id,
            "manual",
            None,
            None,
            reason.trim(),
            "supports",
        )?;
        let proposal_id = record_role_proposal(
            &tx,
            RoleProposal {
                role: "human_reviewer",
                source_type: "semantic_claim",
                source_id: &old_claim_id.to_string(),
                suggested_action: "supersede",
                target_claim_id: Some(old_claim_id),
                evidence_refs: "[\"manual review\"]",
                payload: &serde_json::to_string(&serde_json::json!({ "replacement_claim_id": new_claim_id, "reason": reason.trim() })).map_err(invalid_json)?,
            },
        )?;
        record_policy_decision(
            &tx,
            proposal_id,
            "human_superseded",
            reason.trim(),
            Some(new_claim_id),
            Some("active"),
        )?;
        tx.commit().map_err(database_error)
    }

    pub fn set_claim_review_schedule(
        &self,
        claim_id: i64,
        volatility: &str,
        next_review_at: Option<&str>,
    ) -> io::Result<()> {
        if !matches!(volatility, "stable" | "slow_changing" | "fast_changing") {
            return Err(invalid(
                "volatility must be stable, slow_changing, or fast_changing",
            ));
        }
        if let Some(value) = next_review_at {
            let valid: bool = self
                .connection
                .query_row("SELECT datetime(?1) IS NOT NULL", [value], |row| row.get(0))
                .map_err(database_error)?;
            if !valid {
                return Err(invalid("review date must be a valid ISO date-time"));
            }
        }
        let changed = self
            .connection
            .execute(
                "UPDATE semantic_claims SET volatility = ?2,
                 next_review_at = COALESCE(?3, CASE ?2
                     WHEN 'fast_changing' THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+30 days')
                     WHEN 'slow_changing' THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+180 days')
                     ELSE NULL END)
             WHERE id = ?1 AND retracted_by IS NULL",
                params![claim_id, volatility, next_review_at],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("claim is not active"));
        }
        Ok(())
    }

    pub fn set_claim_temporal_scope(
        &self,
        claim_id: i64,
        valid_from: Option<&str>,
        valid_until: Option<&str>,
    ) -> io::Result<()> {
        if !valid_claim_period(valid_from, valid_until) {
            return Err(invalid(
                "claim dates must be valid ISO dates with a non-reversed range",
            ));
        }
        let changed = self
            .connection
            .execute(
                "UPDATE semantic_claims SET valid_from = ?2, valid_until = ?3
             WHERE id = ?1 AND retracted_by IS NULL",
                params![claim_id, valid_from, valid_until],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("claim is not active"));
        }
        Ok(())
    }

    pub fn recent_maintenance_decisions(
        &self,
        limit: usize,
    ) -> io::Result<Vec<MaintenanceDecision>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT d.id, p.role, p.source_type, p.source_id, p.suggested_action,
                    d.outcome, d.reason, d.resulting_claim_id, d.resulting_state, d.created_at
             FROM policy_decisions d JOIN role_proposals p ON p.id = d.proposal_id
             ORDER BY d.id DESC LIMIT ?1",
            )
            .map_err(database_error)?;
        statement
            .query_map(
                [i64::try_from(limit).map_err(|_| invalid("limit too large"))?],
                |row| {
                    Ok(MaintenanceDecision {
                        id: row.get(0)?,
                        role: row.get(1)?,
                        source_type: row.get(2)?,
                        source_id: row.get(3)?,
                        suggested_action: row.get(4)?,
                        outcome: row.get(5)?,
                        reason: row.get(6)?,
                        resulting_claim_id: row.get(7)?,
                        resulting_state: row.get(8)?,
                        created_at: row.get(9)?,
                    })
                },
            )
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn claim_open_proposals(&self, claim_id: i64) -> io::Result<Vec<OpenRoleProposal>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT p.role, p.suggested_action, COALESCE(d.outcome, 'no decision'),
                    COALESCE(d.reason, 'awaiting policy decision')
             FROM role_proposals p LEFT JOIN policy_decisions d
               ON d.id = (SELECT MAX(d2.id) FROM policy_decisions d2 WHERE d2.proposal_id = p.id)
             WHERE p.target_claim_id = ?1
               AND COALESCE(d.outcome, '') IN ('deferred', 'challenge_applied', 'queued')
             ORDER BY p.id",
            )
            .map_err(database_error)?;
        statement
            .query_map([claim_id], |row| {
                Ok(OpenRoleProposal {
                    role: row.get(0)?,
                    action: row.get(1)?,
                    outcome: row.get(2)?,
                    reason: row.get(3)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn maintenance_work_counts(&self) -> io::Result<MaintenanceWorkCounts> {
        let curator_reviews: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM candidate_claims WHERE status IN ('proposed', 'needs_review')",
            [], |row| row.get(0),
        ).map_err(database_error)?;
        let scholar_reviews: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM scholar_findings WHERE status = 'needs_review'",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let pending_research: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM scholar_jobs WHERE status IN ('pending', 'processing')",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let challenged_claims: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM semantic_claims WHERE status = 'challenged' AND retracted_by IS NULL",
            [], |row| row.get(0),
        ).map_err(database_error)?;
        let due_claims: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM semantic_claims WHERE retracted_by IS NULL
             AND status IN ('active', 'uncertain', 'challenged') AND next_review_at IS NOT NULL
             AND next_review_at <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        Ok(MaintenanceWorkCounts {
            curator_reviews: usize::try_from(curator_reviews).unwrap_or(usize::MAX),
            scholar_reviews: usize::try_from(scholar_reviews).unwrap_or(usize::MAX),
            pending_research: usize::try_from(pending_research).unwrap_or(usize::MAX),
            challenged_claims: usize::try_from(challenged_claims).unwrap_or(usize::MAX),
            due_claims: usize::try_from(due_claims).unwrap_or(usize::MAX),
        })
    }
}
