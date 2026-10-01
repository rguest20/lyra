use super::*;

impl LearningStore {
    pub fn enqueue_research(
        &self,
        conversation_id: &str,
        turn_number: usize,
        question: &str,
    ) -> io::Result<()> {
        self.connection.execute(
            "INSERT INTO scholar_jobs (id, conversation_id, turn_number, question, status, created_at)
             VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, 'pending',
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(conversation_id, turn_number) DO NOTHING",
            params![conversation_id, i64::try_from(turn_number).map_err(|_| invalid("turn number too large"))?, question],
        ).map_err(database_error)?;
        Ok(())
    }

    /// Atomically record Scout's reason and enqueue the corresponding Scholar job.
    /// One question per topic is enough for this first autonomous research pass.
    pub fn enqueue_scout_research(
        &mut self,
        trigger_kind: &str,
        topic_key: &str,
        trigger_ref: &str,
        question: &str,
    ) -> io::Result<bool> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let recent: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM scout_investigations
             WHERE created_at > strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-5 minutes')",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if recent > 0 {
            return Ok(false);
        }
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM scout_investigations
             WHERE trigger_kind = ?1 AND topic_key = ?2)",
                params![trigger_kind, topic_key],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if exists {
            return Ok(false);
        }
        let job_id: String = tx
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
            .map_err(database_error)?;
        tx.execute(
            "INSERT INTO scholar_jobs
             (id, conversation_id, turn_number, question, status, created_at)
             VALUES (?1, ?2, 0, ?3, 'pending', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![job_id, format!("scout:{job_id}"), question],
        )
        .map_err(database_error)?;
        tx.execute(
            "INSERT INTO scout_investigations
             (trigger_kind, topic_key, trigger_ref, question, scholar_job_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![trigger_kind, topic_key, trigger_ref, question, job_id],
        )
        .map_err(database_error)?;
        let evidence_refs = serde_json::to_string(&[trigger_ref]).map_err(invalid_json)?;
        let payload = serde_json::to_string(&serde_json::json!({
            "trigger_kind": trigger_kind,
            "topic_key": topic_key,
            "question": question,
            "scholar_job_id": job_id,
        }))
        .map_err(invalid_json)?;
        let proposal_id = record_role_proposal(
            &tx,
            RoleProposal {
                role: "scout",
                source_type: "scout_investigation",
                source_id: &job_id,
                suggested_action: "queue_research",
                target_claim_id: trigger_ref
                    .strip_prefix("claim:")
                    .and_then(|id| id.parse().ok()),
                evidence_refs: &evidence_refs,
                payload: &payload,
            },
        )?;
        record_policy_decision(
            &tx,
            proposal_id,
            "queued",
            "bounded Scout investigation accepted by queue policy",
            trigger_ref
                .strip_prefix("claim:")
                .and_then(|id| id.parse().ok()),
            Some("research_queued"),
        )?;
        tx.commit().map_err(database_error)?;
        Ok(true)
    }

    pub fn scout_investigations(&self) -> io::Result<Vec<ScoutInvestigation>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT trigger_kind, topic_key, trigger_ref, question, scholar_job_id
             FROM scout_investigations ORDER BY created_at DESC LIMIT 30",
            )
            .map_err(database_error)?;
        statement
            .query_map([], |row| {
                Ok(ScoutInvestigation {
                    trigger_kind: row.get(0)?,
                    topic_key: row.get(1)?,
                    trigger_ref: row.get(2)?,
                    question: row.get(3)?,
                    scholar_job_id: row.get(4)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn scout_topic_exists(&self, trigger_kind: &str, topic_key: &str) -> io::Result<bool> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM scout_investigations
             WHERE trigger_kind = ?1 AND topic_key = ?2)",
                params![trigger_kind, topic_key],
                |row| row.get(0),
            )
            .map_err(database_error)
    }

    pub fn claim_scholar_job(&mut self) -> io::Result<Option<ScholarJob>> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let job = tx.query_row(
            "SELECT id, question, status FROM scholar_jobs WHERE status = 'pending'
             OR (status = 'processing' AND claimed_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-600 seconds'))
             ORDER BY created_at LIMIT 1",
            [],
            |row| Ok(ScholarJob { id: row.get(0)?, question: row.get(1)?, status: row.get(2)? }),
        ).optional().map_err(database_error)?;
        if let Some(job) = &job {
            tx.execute("UPDATE scholar_jobs SET status = 'processing', claimed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), attempts = attempts + 1 WHERE id = ?1", [&job.id]).map_err(database_error)?;
        }
        tx.commit().map_err(database_error)?;
        Ok(job)
    }

    pub fn finish_scholar_job(
        &mut self,
        job_id: &str,
        source_title: &str,
        source_url: &str,
        source_excerpt: &str,
        finding: Option<ScholarFinding>,
        note: Option<&str>,
    ) -> io::Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let mut job_status = "no_result";
        let mut reason = note.map(str::to_owned);
        if let Some(finding) = finding {
            let today: String = tx
                .query_row("SELECT date('now')", [], |row| row.get(0))
                .map_err(database_error)?;
            let question: String = tx
                .query_row(
                    "SELECT question FROM scholar_jobs WHERE id = ?1",
                    [job_id],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            if !matches!(
                finding.source_quality.as_str(),
                "primary" | "reference" | "unknown"
            ) {
                return Err(invalid("invalid Scholar source quality"));
            }
            let time_sensitive = is_time_sensitive(&question, &finding.relation, &finding.object);
            let freshness = if !time_sensitive {
                "not_time_sensitive"
            } else if let Some(updated_at) = finding.source_updated_at.as_deref() {
                let age_days: Option<f64> = tx
                    .query_row(
                        "SELECT julianday('now') - julianday(?1)",
                        [updated_at],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(database_error)?
                    .flatten();
                match age_days {
                    Some(days) if (0.0..=365.0).contains(&days) => "fresh",
                    Some(days) if days > 365.0 => "stale",
                    _ => "unknown",
                }
            } else {
                "unknown"
            };
            let safe_field = |value: &str| {
                !value.trim().is_empty()
                    && value.chars().count() <= 500
                    && !value.chars().any(char::is_control)
                    && !value.contains("<|")
            };
            let supported = safe_field(&finding.evidence_text)
                && source_excerpt.contains(&finding.evidence_text)
                && safe_field(&finding.subject)
                && safe_field(&finding.relation)
                && safe_field(&finding.object)
                && (evidence_supports_claim(&finding)
                    || (finding.relation == "has encyclopedia summary"
                        && canonical(&finding.subject) == canonical(source_title)
                        && finding.object == finding.evidence_text))
                && finding.confidence.is_finite()
                && (0.0..=1.0).contains(&finding.confidence)
                && valid_claim_period(
                    finding.valid_from.as_deref(),
                    finding.valid_until.as_deref(),
                )
                && [
                    finding.valid_from.as_deref(),
                    finding.valid_until.as_deref(),
                ]
                .into_iter()
                .flatten()
                .all(|date| source_excerpt.contains(date));
            let mut semantic_id = None;
            let mut conflicting_claim_id = None;
            let mut finding_status = "needs_review";
            if !supported {
                reason =
                    Some("scholar claim is not directly supported by its source excerpt".into());
            } else if finding.confidence < PROMOTION_CONFIDENCE {
                reason = Some("scholar extraction confidence below promotion threshold".into());
            } else if matches!(canonical(&finding.subject).as_str(), "user" | "the user") {
                reason =
                    Some("web research cannot establish a personal fact about the user".into());
            } else if finding.source_quality == "unknown" {
                reason = Some(
                    "source quality is unknown; corroboration or human review required".into(),
                );
            } else if freshness == "stale" || (time_sensitive && freshness == "unknown") {
                reason = Some(format!(
                    "time-sensitive evidence is {freshness}; refresh or review required"
                ));
            } else if is_consequential(&question, &finding.subject, &finding.relation) {
                reason = Some("consequential topic requires corroboration or human review".into());
            } else {
                let mut statement = tx.prepare(
                        "SELECT id, object, valid_from, valid_until FROM semantic_claims WHERE retracted_by IS NULL
                     AND lower(trim(subject)) = lower(trim(?1))
                     AND lower(trim(relation)) = lower(trim(?2)) ORDER BY id DESC",
                    ).map_err(database_error)?;
                let existing = statement
                    .query_map(params![finding.subject, finding.relation], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    })
                    .map_err(database_error)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(database_error)?;
                drop(statement);
                for (id, object, valid_from, valid_until) in existing {
                    if !claim_periods_overlap(
                        finding.valid_from.as_deref(),
                        finding.valid_until.as_deref(),
                        valid_from.as_deref(),
                        valid_until.as_deref(),
                        &today,
                    ) {
                        continue;
                    }
                    if canonical(&object) == canonical(&finding.object)
                        && valid_from.as_deref() == finding.valid_from.as_deref()
                        && valid_until.as_deref() == finding.valid_until.as_deref()
                    {
                        semantic_id = Some(id);
                        finding_status = "promoted";
                        break;
                    }
                    if canonical(&object) != canonical(&finding.object) {
                        conflicting_claim_id = Some(id);
                        reason = Some(format!("conflicts with overlapping semantic claim #{id}"));
                        break;
                    }
                }
                if semantic_id.is_none() && conflicting_claim_id.is_none() {
                    tx.execute(
                        "INSERT INTO semantic_claims
                            (subject, relation, object, source, source_observation_id, created_at, valid_from, valid_until)
                         VALUES (?1, ?2, ?3, ?4, NULL, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?5, ?6)",
                        params![finding.subject.trim(), finding.relation.trim(), finding.object.trim(), source_url,
                            finding.valid_from, finding.valid_until],
                    ).map_err(database_error)?;
                    semantic_id = Some(tx.last_insert_rowid());
                    finding_status = "promoted";
                }
            }
            tx.execute(
                "INSERT INTO scholar_findings (job_id, source_title, source_url, source_excerpt,
                  subject, relation, object, evidence_text, confidence, status, semantic_claim_id, decision_reason,
                  valid_from, valid_until, source_quality, freshness, source_updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                params![job_id, source_title, source_url, source_excerpt, finding.subject, finding.relation,
                    finding.object, finding.evidence_text, finding.confidence, finding_status, semantic_id, reason,
                    finding.valid_from, finding.valid_until, finding.source_quality, freshness, finding.source_updated_at],
            ).map_err(database_error)?;
            let finding_id = tx.last_insert_rowid();
            let evidence_refs = serde_json::to_string(&[source_url]).map_err(invalid_json)?;
            let payload = serde_json::to_string(&serde_json::json!({
                "subject": finding.subject,
                "relation": finding.relation,
                "object": finding.object,
                "valid_from": finding.valid_from,
                "valid_until": finding.valid_until,
                "confidence": finding.confidence,
                "source_quality": finding.source_quality,
                "freshness": freshness,
                "source_updated_at": finding.source_updated_at,
                "evidence_text": finding.evidence_text,
            }))
            .map_err(invalid_json)?;
            let proposal_id = record_role_proposal(
                &tx,
                RoleProposal {
                    role: "scholar",
                    source_type: "scholar_finding",
                    source_id: &finding_id.to_string(),
                    suggested_action: "add_claim",
                    target_claim_id: semantic_id.or(conflicting_claim_id),
                    evidence_refs: &evidence_refs,
                    payload: &payload,
                },
            )?;
            let outcome = match finding_status {
                "promoted" => "applied",
                "needs_review" if conflicting_claim_id.is_some() => "challenge_applied",
                _ => "deferred",
            };
            record_policy_decision(
                &tx,
                proposal_id,
                outcome,
                reason
                    .as_deref()
                    .unwrap_or("accepted by source-support policy"),
                semantic_id.or(conflicting_claim_id),
                Some(if conflicting_claim_id.is_some() {
                    "challenged"
                } else {
                    finding_status
                }),
            )?;
            if let Some(claim_id) = semantic_id {
                link_claim_evidence(
                    &tx,
                    claim_id,
                    "web",
                    Some(source_url),
                    None,
                    source_excerpt,
                    "supports",
                )?;
                if finding_status == "promoted" {
                    tx.execute(
                        "UPDATE semantic_claims SET status = 'active',
                             checked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                             last_reviewed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                             next_review_at = CASE volatility
                                 WHEN 'fast_changing' THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+30 days')
                                 WHEN 'slow_changing' THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+180 days')
                                 ELSE NULL END
                         WHERE id = ?1 AND retracted_by IS NULL",
                        [claim_id],
                    ).map_err(database_error)?;
                }
            } else if let Some(claim_id) = conflicting_claim_id {
                link_claim_evidence(
                    &tx,
                    claim_id,
                    "web",
                    Some(source_url),
                    None,
                    source_excerpt,
                    "contradicts",
                )?;
                tx.execute(
                    "UPDATE semantic_claims SET status = 'challenged' WHERE id = ?1
                     AND status IN ('active', 'uncertain') AND retracted_by IS NULL",
                    [claim_id],
                )
                .map_err(database_error)?;
            }
            job_status = finding_status;
        }
        let changed = tx
            .execute(
                "UPDATE scholar_jobs SET status = ?2, last_error = ?3, claimed_at = NULL
             WHERE id = ?1 AND status = 'processing'",
                params![job_id, job_status, reason],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("scholar job is not processing"));
        }
        tx.commit().map_err(database_error)
    }

    pub fn hold_scholar_job(&self, job_id: &str, reason: &str) -> io::Result<()> {
        self.connection.execute(
            "UPDATE scholar_jobs SET status = 'needs_review', last_error = ?2, claimed_at = NULL
             WHERE id = ?1 AND status = 'processing'",
            params![job_id, reason],
        ).map_err(database_error)?;
        Ok(())
    }

    pub fn scholar_jobs(&self) -> io::Result<Vec<(ScholarJob, Option<String>)>> {
        let mut statement = self.connection.prepare(
            "SELECT id, question, status, last_error FROM scholar_jobs ORDER BY created_at DESC LIMIT 30"
        ).map_err(database_error)?;
        statement
            .query_map([], |row| {
                Ok((
                    ScholarJob {
                        id: row.get(0)?,
                        question: row.get(1)?,
                        status: row.get(2)?,
                    },
                    row.get(3)?,
                ))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn scholar_findings(&self, job_id: &str) -> io::Result<Vec<StoredScholarFinding>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, source_url, subject, relation, object, evidence_text,
                    confidence, status, decision_reason, valid_from, valid_until,
                    source_quality, freshness, source_updated_at
             FROM scholar_findings WHERE job_id = ?1 ORDER BY id",
            )
            .map_err(database_error)?;
        statement
            .query_map([job_id], |row| {
                Ok(StoredScholarFinding {
                    id: row.get(0)?,
                    source_url: row.get(1)?,
                    subject: row.get(2)?,
                    relation: row.get(3)?,
                    object: row.get(4)?,
                    evidence_text: row.get(5)?,
                    confidence: row.get(6)?,
                    status: row.get(7)?,
                    decision_reason: row.get(8)?,
                    valid_from: row.get(9)?,
                    valid_until: row.get(10)?,
                    source_quality: row.get(11)?,
                    freshness: row.get(12)?,
                    source_updated_at: row.get(13)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn review_scholar_finding(
        &mut self,
        finding_id: i64,
        accept: bool,
        reason: &str,
    ) -> io::Result<()> {
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
        let finding = tx
            .query_row(
                "SELECT job_id, source_url, source_excerpt, subject, relation, object, status,
                    valid_from, valid_until
             FROM scholar_findings WHERE id = ?1",
                [finding_id],
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
                        row.get::<_, Option<String>>(8)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("no scholar finding with that ID"))?;
        let (
            job_id,
            source_url,
            source_excerpt,
            subject,
            relation,
            object,
            status,
            valid_from,
            valid_until,
        ) = finding;
        if status != "needs_review" {
            return Err(invalid("scholar finding is already resolved"));
        }
        let semantic_id = if accept {
            let mut statement = tx
                .prepare(
                    "SELECT id, subject, relation, object, valid_from, valid_until
                 FROM semantic_claims WHERE retracted_by IS NULL",
                )
                .map_err(database_error)?;
            let current = statement
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
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?;
            drop(statement);
            let today: String = tx
                .query_row("SELECT date('now')", [], |row| row.get(0))
                .map_err(database_error)?;
            let equivalent = current
                .iter()
                .find(|(_, s, r, o, from, until)| {
                    canonical(s) == canonical(&subject)
                        && canonical(r) == canonical(&relation)
                        && canonical(o) == canonical(&object)
                        && from.as_deref() == valid_from.as_deref()
                        && until.as_deref() == valid_until.as_deref()
                })
                .map(|(id, _, _, _, _, _)| *id);
            if let Some(id) = equivalent {
                Some(id)
            } else {
                let conflicting = current
                    .iter()
                    .find(|(_, s, r, o, from, until)| {
                        canonical(s) == canonical(&subject)
                            && canonical(r) == canonical(&relation)
                            && canonical(o) != canonical(&object)
                            && claim_periods_overlap(
                                valid_from.as_deref(),
                                valid_until.as_deref(),
                                from.as_deref(),
                                until.as_deref(),
                                &today,
                            )
                    })
                    .map(|(id, _, _, _, from, until)| (*id, from.clone(), until.clone()));
                let supersedes_id = if let Some((id, old_from, old_until)) = &conflicting {
                    if old_from.as_deref() == valid_from.as_deref()
                        && old_until.as_deref() == valid_until.as_deref()
                    {
                        tx.execute(
                            "UPDATE semantic_claims SET retracted_by = ?2, status = 'superseded' WHERE id = ?1",
                            params![id, format!("superseded by reviewed scholar finding #{finding_id}: {}", reason.trim())],
                        ).map_err(database_error)?;
                        Some(*id)
                    } else {
                        // A partial time overlap does not invalidate the old claim's
                        // entire period. Keep it active but challenged for review.
                        tx.execute(
                            "UPDATE semantic_claims SET status = 'challenged' WHERE id = ?1",
                            [id],
                        )
                        .map_err(database_error)?;
                        None
                    }
                } else {
                    None
                };
                tx.execute(
                    "INSERT INTO semantic_claims
                        (subject, relation, object, source, supersedes_id, source_observation_id, created_at, valid_from, valid_until)
                     VALUES (?1, ?2, ?3, ?4, ?5, NULL, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?6, ?7)",
                    params![subject, relation, object, source_url, supersedes_id, valid_from, valid_until],
                ).map_err(database_error)?;
                Some(tx.last_insert_rowid())
            }
        } else {
            None
        };
        tx.execute(
            "UPDATE scholar_findings SET status = ?2, semantic_claim_id = ?3, decision_reason = ?4 WHERE id = ?1",
            params![finding_id, if accept { "promoted" } else { "rejected" }, semantic_id, reason.trim()],
        ).map_err(database_error)?;
        let proposal_id: i64 = tx.query_row(
            "SELECT id FROM role_proposals WHERE source_type = 'scholar_finding' AND source_id = ?1 ORDER BY id DESC LIMIT 1",
            [finding_id.to_string()], |row| row.get(0),
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
        tx.execute(
            "UPDATE scholar_jobs SET status = ?2, last_error = NULL WHERE id = ?1",
            params![job_id, if accept { "promoted" } else { "no_result" }],
        )
        .map_err(database_error)?;
        if let Some(claim_id) = semantic_id {
            link_claim_evidence(
                &tx,
                claim_id,
                "web",
                Some(&source_url),
                None,
                &source_excerpt,
                "supports",
            )?;
            tx.execute(
                "UPDATE semantic_claims SET status = 'active',
                     checked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                     last_reviewed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                     next_review_at = CASE volatility
                         WHEN 'fast_changing' THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+30 days')
                         WHEN 'slow_changing' THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+180 days')
                         ELSE NULL END
                 WHERE id = ?1 AND retracted_by IS NULL",
                [claim_id],
            ).map_err(database_error)?;
        }
        tx.commit().map_err(database_error)
    }
}
