use super::*;
use serde_json::Value;

impl AssistantReplyAttention {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            reply_id: row.get(0)?,
            status: row.get(1)?,
            error: row.get(2)?,
        })
    }
}

impl LearningStore {
    pub fn stage_assistant_reply(
        &self,
        conversation_id: &str,
        turn_number: usize,
        reply_text: &str,
    ) -> io::Result<()> {
        let turn_number =
            i64::try_from(turn_number).map_err(|_| invalid("turn number too large"))?;
        self.connection
            .execute(
                "INSERT INTO assistant_replies
             (id, conversation_id, turn_number, reply_text, status, created_at)
             VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, 'pending',
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(conversation_id, turn_number) DO NOTHING",
                params![conversation_id, turn_number, reply_text],
            )
            .map_err(database_error)?;
        Ok(())
    }

    pub fn claim_assistant_reply(&mut self) -> io::Result<Option<AssistantReplyJob>> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let job = tx.query_row(
            "SELECT id, reply_text, status FROM assistant_replies
             WHERE status = 'pending' OR
               (status = 'processing' AND claimed_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-600 seconds'))
             ORDER BY created_at, rowid LIMIT 1",
            [],
            |row| Ok(AssistantReplyJob { id: row.get(0)?, reply_text: row.get(1)?, status: row.get(2)? }),
        ).optional().map_err(database_error)?;
        if let Some(job) = &job {
            tx.execute(
                "UPDATE assistant_replies SET status = 'processing',
                 claimed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
                [&job.id],
            )
            .map_err(database_error)?;
        }
        tx.commit().map_err(database_error)?;
        Ok(job)
    }

    pub fn finish_assistant_reply(
        &mut self,
        job_id: &str,
        judgments: &[ClaimJudgment],
    ) -> io::Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        for judgment in judgments {
            let confidence_before = ASSISTANT_BASE_CONFIDENCE;
            let confidence_after = if judgment.verdict == "challenged" {
                confidence_before * SEVERE_CHALLENGE_MULTIPLIER
            } else {
                confidence_before
            };
            tx.execute(
                "INSERT INTO assistant_claims
                 (reply_id, subject, relation, object, evidence_text, extraction_confidence,
                  confidence_before, confidence_after, verdict, challenge_type, reason, evidence_refs, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                         strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![job_id, judgment.claim.subject, judgment.claim.relation,
                    judgment.claim.object, judgment.claim.evidence_text,
                    judgment.claim.extraction_confidence, confidence_before,
                    confidence_after, judgment.verdict, judgment.challenge_type,
                    judgment.reason, serde_json::to_string(&judgment.evidence_refs).map_err(invalid_json)?],
            ).map_err(database_error)?;
            let judgment_id = tx.last_insert_rowid();
            let payload = serde_json::to_string(&serde_json::json!({
                "subject": judgment.claim.subject,
                "relation": judgment.claim.relation,
                "object": judgment.claim.object,
                "verdict": judgment.verdict,
                "challenge_type": judgment.challenge_type,
                "reason": judgment.reason,
                "confidence_before": confidence_before,
                "confidence_after": confidence_after,
            }))
            .map_err(invalid_json)?;
            let proposal_id = record_role_proposal(
                &tx,
                RoleProposal {
                    role: "curmudgeon",
                    source_type: "assistant_claim",
                    source_id: &judgment_id.to_string(),
                    suggested_action: if judgment.verdict == "challenged" {
                        "reduce_usable_confidence"
                    } else {
                        "retain_uncertain"
                    },
                    target_claim_id: None,
                    evidence_refs: &serde_json::to_string(&judgment.evidence_refs)
                        .map_err(invalid_json)?,
                    payload: &payload,
                },
            )?;
            record_policy_decision(
                &tx,
                proposal_id,
                if judgment.verdict == "challenged" {
                    "applied"
                } else {
                    "no_change"
                },
                &judgment.reason,
                None,
                Some(if judgment.verdict == "challenged" {
                    "challenged"
                } else {
                    "inconclusive"
                }),
            )?;
        }
        let status = if judgments.iter().any(|item| item.verdict == "challenged") {
            "challenged"
        } else if judgments.is_empty() {
            "no_claims"
        } else {
            "inconclusive"
        };
        let changed = tx
            .execute(
                "UPDATE assistant_replies SET status = ?2, claimed_at = NULL, last_error = NULL
             WHERE id = ?1 AND status = 'processing'",
                params![job_id, status],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("assistant reply is not processing"));
        }
        tx.commit().map_err(database_error)
    }

    /// Reconstruct the exchange as it stood when this reply was delivered.
    /// Future turns and conversations are excluded from its advisory audit.
    pub fn assistant_reply_context(&self, job_id: &str) -> io::Result<Value> {
        let (conversation_id, turn): (String, i64) = self
            .connection
            .query_row(
                "SELECT conversation_id, turn_number FROM assistant_replies WHERE id=?1",
                [job_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(database_error)?;
        let latest: Option<String> = self
            .connection
            .query_row(
                "SELECT raw_text FROM observations WHERE conversation_id=?1 AND turn_number=?2
             ORDER BY created_at DESC, rowid DESC LIMIT 1",
                params![conversation_id, turn],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT role, content FROM (
                SELECT turn_number, 0 AS sequence, 'user' AS role, raw_text AS content
                FROM observations WHERE conversation_id=?1 AND turn_number<?2
                UNION ALL
                SELECT turn_number, 1, 'assistant', reply_text
                FROM assistant_replies WHERE conversation_id=?1 AND turn_number<?2
             ) ORDER BY turn_number DESC, sequence DESC LIMIT 16",
            )
            .map_err(database_error)?;
        let mut recent = statement.query_map(params![conversation_id, turn], |row| {
            Ok(serde_json::json!({"role":row.get::<_, String>(0)?,"content":row.get::<_, String>(1)?}))
        }).map_err(database_error)?.collect::<Result<Vec<_>, _>>().map_err(database_error)?;
        recent.reverse();
        Ok(serde_json::json!({"recent_conversation":recent,"latest_user_message":latest}))
    }

    /// Keep the conversation-quality verdict in the diagnostic proposal ledger;
    /// it cannot approve factual claims or replace text already delivered.
    pub fn record_conversation_review(
        &mut self,
        job_id: &str,
        verdict: &str,
        reason: &str,
        model_id: &str,
        context: &Value,
    ) -> io::Result<()> {
        if !matches!(verdict, "accept" | "revise" | "uncertain")
            || (verdict != "accept" && reason.trim().is_empty())
        {
            return Err(invalid("invalid conversation audit verdict"));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let processing: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM assistant_replies WHERE id=?1 AND status='processing')",
            [job_id], |row| row.get(0),
        ).map_err(database_error)?;
        if !processing {
            return Err(invalid("assistant reply is not processing"));
        }
        let payload = serde_json::json!({
            "verdict":verdict,"reason":reason,"model_id":model_id,
            "context":context,"advisory_only":true,
        })
        .to_string();
        let id = record_role_proposal(
            &tx,
            RoleProposal {
                role: "curmudgeon",
                source_type: "assistant_reply_quality",
                source_id: job_id,
                suggested_action: if verdict == "accept" {
                    "retain_conversation_review"
                } else {
                    "flag_conversation_reply"
                },
                target_claim_id: None,
                evidence_refs: "[]",
                payload: &payload,
            },
        )?;
        record_policy_decision(
            &tx,
            id,
            "no_change",
            "Advisory conversation audit; no factual approval or replacement of delivered text.",
            None,
            None,
        )?;
        tx.commit().map_err(database_error)
    }

    pub fn hold_assistant_reply(&self, job_id: &str, reason: &str) -> io::Result<()> {
        self.connection.execute(
            "UPDATE assistant_replies SET status = 'needs_review', claimed_at = NULL, last_error = ?2
             WHERE id = ?1 AND status = 'processing'",
            params![job_id, reason],
        ).map_err(database_error)?;
        Ok(())
    }

    pub fn assistant_claims(&self, limit: usize) -> io::Result<Vec<StoredAssistantClaim>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, reply_id, subject, relation, object, evidence_text,
                    verdict, reason, evidence_refs, confidence_before, confidence_after
             FROM assistant_claims ORDER BY id DESC LIMIT ?1",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([sqlite_limit(limit)?], read_assistant_claim)
            .map_err(database_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(database_error)
    }

    pub(crate) fn assistant_reply_attention(&self) -> io::Result<Vec<AssistantReplyAttention>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, status, COALESCE(last_error, '') FROM assistant_replies
             WHERE status IN ('pending', 'processing', 'needs_review')
             ORDER BY created_at DESC LIMIT 20",
            )
            .map_err(database_error)?;
        statement
            .query_map([], AssistantReplyAttention::from_row)
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn relevant_assistant_challenges(
        &self,
        query: &str,
        limit: usize,
    ) -> io::Result<Vec<StoredAssistantClaim>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let terms = relevant_words(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, reply_id, subject, relation, object, evidence_text,
                    verdict, reason, evidence_refs, confidence_before, confidence_after
             FROM assistant_claims WHERE verdict = 'challenged'",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([], read_assistant_claim)
            .map_err(database_error)?;
        let mut claims = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
            .into_iter()
            .filter_map(|claim| {
                let content = format!("{} {} {}", claim.subject, claim.relation, claim.object);
                let score = lexical_overlap(&terms, &relevant_words(&content));
                (score > 0).then_some((score, claim))
            })
            .collect::<Vec<_>>();
        claims.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(claims
            .into_iter()
            .take(limit)
            .map(|(_, claim)| claim)
            .collect())
    }

    pub fn active_semantic_claim_ids(&self) -> io::Result<std::collections::HashSet<i64>> {
        let mut statement = self
            .connection
            .prepare("SELECT id FROM semantic_claims WHERE retracted_by IS NULL")
            .map_err(database_error)?;
        statement
            .query_map([], |row| row.get(0))
            .map_err(database_error)?
            .collect::<Result<std::collections::HashSet<i64>, _>>()
            .map_err(database_error)
    }

    pub fn utc_date(&self) -> io::Result<String> {
        current_sql_date(&self.connection)
    }

    pub fn latest_reply_challenges(
        &self,
        conversation_id: &str,
    ) -> io::Result<Vec<StoredAssistantClaim>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT c.id, c.reply_id, c.subject, c.relation, c.object, c.evidence_text,
                    c.verdict, c.reason, c.evidence_refs, c.confidence_before, c.confidence_after
             FROM assistant_claims c WHERE c.verdict = 'challenged' AND c.reply_id = (
                SELECT id FROM assistant_replies WHERE conversation_id = ?1
                ORDER BY turn_number DESC LIMIT 1
             ) ORDER BY c.id",
            )
            .map_err(database_error)?;
        statement
            .query_map([conversation_id], read_assistant_claim)
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }
}
