use super::*;

impl LearningStore {
    /// Import the JSON map without changing it. Re-running is safe: matching
    /// rows are accepted, while an ID with different content aborts the import.
    pub fn import_knowledge(&mut self, map: &KnowledgeMap) -> io::Result<ImportSummary> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let mut inserted = 0;
        let mut already_present = 0;

        for claim in map.entries() {
            match get_claim(&tx, claim.id)? {
                Some(existing) if claims_equal(&existing, claim) => already_present += 1,
                Some(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "SQLite already has claim #{} with different content; import stopped",
                            claim.id
                        ),
                    ));
                }
                None => {
                    let id = i64::try_from(claim.id)
                        .map_err(|_| invalid("claim ID exceeds SQLite's integer range"))?;
                    let supersedes =
                        claim
                            .supersedes
                            .map(i64::try_from)
                            .transpose()
                            .map_err(|_| {
                                invalid("superseded claim ID exceeds SQLite's integer range")
                            })?;
                    tx.execute(
                        "INSERT INTO semantic_claims
                            (id, subject, relation, object, source, supersedes_id,
                             retracted_by, source_observation_id, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)",
                        params![
                            id,
                            claim.subject,
                            claim.relation,
                            claim.object,
                            claim.source,
                            supersedes,
                            claim.retracted_by
                        ],
                    )
                    .map_err(database_error)?;
                    inserted += 1;
                }
            }
        }

        tx.commit().map_err(database_error)?;
        Ok(ImportSummary {
            inserted,
            already_present,
        })
    }

    /// Persist one user turn before generation. Retrying the same turn is
    /// idempotent and returns its original observation ID.
    pub fn stage_observation(
        &self,
        conversation_id: &str,
        turn_number: usize,
        raw_text: &str,
    ) -> io::Result<String> {
        let turn_number = i64::try_from(turn_number)
            .map_err(|_| invalid("conversation turn exceeds SQLite's integer range"))?;
        self.connection
            .execute(
                "INSERT INTO observations
                    (id, conversation_id, turn_number, speaker, raw_text,
                     created_at, status)
                 VALUES (lower(hex(randomblob(16))), ?1, ?2, 'user', ?3,
                         strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'pending')
                 ON CONFLICT(conversation_id, turn_number, raw_text) DO NOTHING",
                params![conversation_id, turn_number, raw_text],
            )
            .map_err(database_error)?;
        self.connection
            .query_row(
                "SELECT id FROM observations
                 WHERE conversation_id = ?1 AND turn_number = ?2 AND raw_text = ?3",
                params![conversation_id, turn_number, raw_text],
                |row| row.get(0),
            )
            .map_err(database_error)
    }

    /// Find recent unprocessed observations from this conversation that share
    /// meaningful words with the current question.
    pub fn relevant_observations(
        &self,
        conversation_id: &str,
        query: &str,
        exclude_id: &str,
        limit: usize,
    ) -> io::Result<Vec<Observation>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let query_words = relevant_words(query);
        if query_words.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT o.id, o.turn_number, o.raw_text, o.status, o.created_at,
                        COALESCE((SELECT group_concat(c.subject || ' ' || c.relation || ' ' || c.object, ' ')
                                  FROM candidate_claims c
                                  WHERE c.observation_id = o.id
                                    AND c.status IN ('proposed', 'needs_review')
                                    AND lower(trim(c.subject)) IN ('user', 'the user')), '')
                 FROM observations o
                 WHERE o.conversation_id = ?1
                   AND o.id != ?2
                   AND o.status IN ('pending', 'processing', 'needs_review')
                 ORDER BY o.turn_number DESC LIMIT 256",
            )
            .map_err(database_error)?;
        let candidates = statement
            .query_map(params![conversation_id, exclude_id], |row| {
                let turn_number: i64 = row.get(1)?;
                Ok((
                    Observation {
                        id: row.get(0)?,
                        conversation_id: conversation_id.to_owned(),
                        turn_number: usize::try_from(turn_number).map_err(|_| {
                            rusqlite::Error::IntegralValueOutOfRange(1, turn_number)
                        })?,
                        raw_text: row.get(2)?,
                        status: row.get(3)?,
                        created_at: row.get(4)?,
                    },
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(database_error)?;
        let mut scored = Vec::new();
        for result in candidates {
            let (observation, candidate_text) = result.map_err(database_error)?;
            let score = lexical_overlap(&relevant_words(&observation.raw_text), &query_words).max(
                lexical_overlap(&relevant_words(&candidate_text), &query_words),
            );
            if score > 0 {
                scored.push((score, observation));
            }
        }
        scored.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| right.turn_number.cmp(&left.turn_number))
        });
        Ok(scored
            .into_iter()
            .take(limit)
            .map(|(_, observation)| observation)
            .collect())
    }

    /// Claim one queued observation. A worker that disappears leaves a lease
    /// which becomes available again after `lease_seconds`.
    pub fn claim_next_observation(
        &mut self,
        lease_seconds: u64,
    ) -> io::Result<Option<Observation>> {
        let lease_modifier = format!("-{} seconds", lease_seconds);
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let candidate = tx
            .query_row(
                "SELECT id, conversation_id, turn_number, raw_text, status, created_at
                 FROM observations
                 WHERE status = 'pending'
                    OR (status = 'processing' AND
                        (claimed_at IS NULL OR claimed_at <
                         strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?1)))
                 ORDER BY created_at, turn_number
                 LIMIT 1",
                [&lease_modifier],
                |row| {
                    let turn_number: i64 = row.get(2)?;
                    Ok(Observation {
                        id: row.get(0)?,
                        conversation_id: row.get(1)?,
                        turn_number: usize::try_from(turn_number).map_err(|_| {
                            rusqlite::Error::IntegralValueOutOfRange(2, turn_number)
                        })?,
                        raw_text: row.get(3)?,
                        status: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(database_error)?;
        if let Some(observation) = candidate {
            tx.execute(
                "UPDATE observations
                 SET status = 'processing',
                     claimed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                     retry_count = retry_count + 1
                 WHERE id = ?1",
                [&observation.id],
            )
            .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
            Ok(Some(Observation {
                status: "processing".into(),
                ..observation
            }))
        } else {
            tx.commit().map_err(database_error)?;
            Ok(None)
        }
    }

    /// Curate extracted candidates, retain provenance, and finish the
    /// observation atomically.
    pub fn finish_extraction(
        &mut self,
        observation_id: &str,
        claims: &[CandidateClaim],
        review_reason: Option<&str>,
    ) -> io::Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let mut review_reasons = Vec::new();
        let observation_text: String = tx
            .query_row(
                "SELECT raw_text FROM observations WHERE id = ?1",
                [observation_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        for claim in claims {
            let mut claim = claim.clone();
            claim.subject = normalize(&claim.subject);
            claim.relation = normalize(&claim.relation);
            claim.object = normalize(&claim.object);
            let claim_period_valid =
                valid_claim_period(claim.valid_from.as_deref(), claim.valid_until.as_deref())
                    && [claim.valid_from.as_deref(), claim.valid_until.as_deref()]
                        .into_iter()
                        .flatten()
                        .all(|date| observation_text.contains(date));
            let semantic_id = if claim.status == "rejected" {
                None
            } else if !claim_period_valid {
                claim.status = "needs_review".into();
                claim.decision_reason =
                    Some("claim dates are invalid or not explicit in the user observation".into());
                review_reasons.push(claim.decision_reason.clone().unwrap());
                None
            } else if claim.status == "proposed" && claim.confidence >= PROMOTION_CONFIDENCE {
                match curate_candidate(&tx, observation_id, &mut claim)? {
                    Ok(id) => Some(id),
                    Err(reason) => {
                        claim.status = "needs_review".into();
                        claim.decision_reason = Some(reason.clone());
                        review_reasons.push(reason);
                        None
                    }
                }
            } else {
                claim.status = "needs_review".into();
                claim.decision_reason.get_or_insert_with(|| {
                    format!("confidence below promotion threshold of {PROMOTION_CONFIDENCE:.2}")
                });
                review_reasons.push(claim.decision_reason.clone().unwrap());
                None
            };
            let candidate_id: String = tx
                .query_row(
                    "INSERT INTO candidate_claims
                    (id, observation_id, subject, relation, object, evidence_text,
                     context_message_ids, resolution, confidence, status, decision_reason,
                     semantic_claim_id, valid_from, valid_until)
                 VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, ?4, ?5, ?6,
                         ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 RETURNING id",
                    params![
                        observation_id,
                        claim.subject,
                        claim.relation,
                        claim.object,
                        claim.evidence_text,
                        serde_json::to_string(&claim.context_message_ids).map_err(invalid_json)?,
                        claim.resolution,
                        claim.confidence,
                        claim.status,
                        claim.decision_reason,
                        semantic_id,
                        claim.valid_from,
                        claim.valid_until
                    ],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            let evidence_refs = serde_json::to_string(&[observation_id]).map_err(invalid_json)?;
            let payload = serde_json::to_string(&serde_json::json!({
                "subject": claim.subject,
                "relation": claim.relation,
                "object": claim.object,
                "valid_from": claim.valid_from,
                "valid_until": claim.valid_until,
                "confidence": claim.confidence,
                "evidence_text": claim.evidence_text,
            }))
            .map_err(invalid_json)?;
            let proposal_id = record_role_proposal(
                &tx,
                RoleProposal {
                    role: "curator",
                    source_type: "candidate_claim",
                    source_id: &candidate_id,
                    suggested_action: "add_claim",
                    target_claim_id: semantic_id,
                    evidence_refs: &evidence_refs,
                    payload: &payload,
                },
            )?;
            record_policy_decision(
                &tx,
                proposal_id,
                match claim.status.as_str() {
                    "promoted" => "applied",
                    "rejected" => "rejected",
                    _ => "deferred",
                },
                claim
                    .decision_reason
                    .as_deref()
                    .unwrap_or("accepted by deterministic curation policy"),
                semantic_id,
                Some(match claim.status.as_str() {
                    "promoted" => "active",
                    "rejected" => "rejected",
                    _ => "needs_review",
                }),
            )?;
            if claim.status == "promoted" {
                attach_equivalent_sources(&tx, semantic_id.unwrap(), &candidate_id, &claim)?;
            }
        }
        if let Some(reason) = review_reason {
            review_reasons.push(reason.to_owned());
        }
        let review_summary = (!review_reasons.is_empty()).then(|| {
            review_reasons
                .join("; ")
                .chars()
                .take(500)
                .collect::<String>()
        });
        let (status, reason) = if let Some(reason) = review_summary.as_deref() {
            ("needs_review", Some(reason))
        } else if claims.is_empty() || claims.iter().all(|claim| claim.status == "rejected") {
            (
                "rejected",
                Some("AI reviewer rejected all extracted claims"),
            )
        } else {
            ("promoted", None)
        };
        let changed = tx
            .execute(
                "UPDATE observations
                 SET status = ?2, last_error = ?3, claimed_at = NULL
                 WHERE id = ?1 AND status = 'processing'",
                params![observation_id, status, reason],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("observation is not currently claimed by a worker"));
        }
        tx.commit().map_err(database_error)
    }
}
