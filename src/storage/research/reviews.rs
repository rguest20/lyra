//! Scholar source assessments, provenance decisions and finding review.
use super::*;

struct ScholarFindingReview {
    job_id: String,
    source_url: String,
    source_title: String,
    evidence: FindingEvidence,
    subject: String,
    relation: String,
    object: String,
    valid_from: Option<String>,
    valid_until: Option<String>,
    proposition: PropositionMetadata,
}

struct ExistingSemanticClaim {
    subject: String,
    relation: String,
    scope: super::SemanticClaimScope,
}

struct FindingEvidence {
    status: String,
    source_excerpt: String,
    evidence_text: String,
}

impl FindingEvidence {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            status: row.get(0)?,
            source_excerpt: row.get(1)?,
            evidence_text: row.get(2)?,
        })
    }

    fn contains_quote(&self, quote: &str) -> bool {
        self.source_excerpt.contains(quote)
    }

    fn require_pending_review(&self, error: &'static str) -> io::Result<()> {
        if self.status == "needs_review" {
            Ok(())
        } else {
            Err(invalid(error))
        }
    }

    fn require_evidence_quote(&self) -> io::Result<()> {
        if self.contains_quote(&self.evidence_text) {
            Ok(())
        } else {
            Err(invalid(
                "finding evidence is not present in its stored source passage",
            ))
        }
    }
}

struct SourceExpertiseFinding {
    evidence: FindingEvidence,
    confidence: f64,
}

struct SourceAssessmentState {
    topic_class: String,
    direct_support: Option<i64>,
    independence_verified: bool,
    gate_status: String,
}

impl ScholarFindingReview {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            evidence: FindingEvidence::from_row(row)?,
            job_id: row.get(3)?,
            source_url: row.get(4)?,
            subject: row.get(5)?,
            relation: row.get(6)?,
            object: row.get(7)?,
            valid_from: row.get(8)?,
            valid_until: row.get(9)?,
            proposition: PropositionMetadata::from_storage_row(row, 10)?,
            source_title: row.get(15)?,
        })
    }
}

struct SourceOriginFinding {
    evidence: FindingEvidence,
    source_kind: String,
    proposition: super::ScholarPropositionScope,
}

impl LearningStore {
    pub fn review_scholar_source_expertise(
        &mut self,
        finding_id: i64,
        source_kind: &str,
        suitable_for_topic: bool,
        published_at: Option<&str>,
        rationale: &str,
    ) -> io::Result<()> {
        self.review_scholar_source_expertise_with_method(
            finding_id,
            source_kind,
            suitable_for_topic,
            published_at,
            rationale,
            "human",
        )
    }

    pub fn review_scholar_source_expertise_ai(
        &mut self,
        finding_id: i64,
        source_kind: &str,
        suitable_for_topic: bool,
        published_at: Option<&str>,
        rationale: &str,
    ) -> io::Result<()> {
        self.review_scholar_source_expertise_with_method(
            finding_id,
            source_kind,
            suitable_for_topic,
            published_at,
            rationale,
            "automatic",
        )
    }

    fn review_scholar_source_expertise_with_method(
        &mut self,
        finding_id: i64,
        source_kind: &str,
        suitable_for_topic: bool,
        published_at: Option<&str>,
        rationale: &str,
        review_method: &str,
    ) -> io::Result<()> {
        if !matches!(source_kind, "primary" | "secondary") {
            return Err(invalid("source kind must be primary or secondary"));
        }
        require_printable_field(
            rationale,
            500,
            "source review reason must contain 1 to 500 printable characters",
        )?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let finding = tx
            .query_row(
                "SELECT status, source_excerpt, evidence_text, confidence
                 FROM scholar_findings WHERE id = ?1",
                [finding_id],
                |row| {
                    Ok(SourceExpertiseFinding {
                        evidence: FindingEvidence::from_row(row)?,
                        confidence: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("no scholar finding with that ID"))?;
        finding.evidence.require_pending_review(
            "source expertise can only be reviewed for a finding awaiting review",
        )?;
        finding.evidence.require_evidence_quote()?;
        let published_at = if let Some(date) = published_at {
            let normalized: Option<String> = tx
                .query_row("SELECT date(?1)", [date], |row| row.get(0))
                .map_err(database_error)?;
            Some(normalized.ok_or_else(|| invalid("publication date must be a valid ISO date"))?)
        } else {
            None
        };
        let assessment = tx
            .query_row(
                "SELECT topic_class, direct_support, independence_verified, gate_status
                 FROM scholar_source_assessments WHERE finding_id = ?1",
                [finding_id],
                |row| {
                    Ok(SourceAssessmentState {
                        topic_class: row.get(0)?,
                        direct_support: row.get(1)?,
                        independence_verified: row.get::<_, i64>(2)? != 0,
                        gate_status: row.get(3)?,
                    })
                },
            )
            .map_err(database_error)?;
        tx.execute(
            "INSERT INTO scholar_source_expertise_reviews
                (finding_id, source_kind, suitable_for_topic, published_at,
                 evidence_text, rationale, reviewed_at, review_method)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6,
                strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?7)",
            params![
                finding_id,
                source_kind,
                i64::from(suitable_for_topic),
                published_at,
                finding.evidence.evidence_text,
                rationale,
                review_method,
            ],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_findings SET source_quality = ?2
             WHERE id = ?1",
            params![
                finding_id,
                if suitable_for_topic && source_kind == "primary" {
                    "primary"
                } else if suitable_for_topic {
                    "reference"
                } else {
                    "unknown"
                },
            ],
        )
        .map_err(database_error)?;
        let is_recent = crate::storage::publication_date_is_recent(&tx, published_at.as_deref())?;
        let (gate_status, topical_expertise, gate_reason) = if !suitable_for_topic {
            (
                "pending_review",
                "reviewer_found_topic_mismatch",
                "reviewer found the source unsuitable for this claim's topic",
            )
        } else if assessment.direct_support != Some(1) {
            (
                "unsupported",
                "reviewer_verified_for_claim",
                "source suitability was reviewed, but the stored passage does not directly support this claim",
            )
        } else if finding.confidence < PROMOTION_CONFIDENCE {
            (
                "pending_review",
                "reviewer_verified_for_claim",
                "source suitability was reviewed, but extraction confidence remains below the threshold",
            )
        } else if assessment.topic_class == "sensitive" {
            (
                "pending_review",
                "reviewer_verified_for_claim",
                "sensitive claims still require explicit finding review",
            )
        } else if assessment.topic_class == "recent" && !is_recent {
            (
                "unverified_temporal",
                "reviewer_verified_for_claim",
                "recent claim needs a reviewer-confirmed publication date within 12 months",
            )
        } else if source_kind == "primary" {
            (
                "eligible",
                "reviewer_verified_claim_specific_primary",
                "reviewer verified this primary source's expertise for the claim topic and the passage directly supports the claim",
            )
        } else if assessment.independence_verified && assessment.gate_status == "eligible" {
            (
                "eligible",
                "reviewer_verified_for_claim",
                "source suitability is verified and source independence was already established",
            )
        } else {
            (
                "awaiting_corroboration",
                "reviewer_verified_for_claim",
                "source suitability is verified; a secondary source still needs verified independent corroboration",
            )
        };
        let date_reason = published_at
            .as_deref()
            .map(|date| format!(" Reviewer-confirmed publication date: {date}."))
            .unwrap_or_default();
        tx.execute(
            "UPDATE scholar_source_assessments SET source_kind = ?2,
                topical_expertise = ?3, published_at = ?4, gate_status = ?5,
                reason = ?6 WHERE finding_id = ?1",
            params![
                finding_id,
                source_kind,
                topical_expertise,
                published_at,
                gate_status,
                format!("{gate_reason}.{date_reason} Review reason: {rationale}"),
            ],
        )
        .map_err(database_error)?;
        tx.commit().map_err(database_error)
    }

    pub fn scholar_source_expertise_reviews(
        &self,
        finding_id: i64,
    ) -> io::Result<Vec<ScholarSourceExpertiseReview>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, finding_id, source_kind, suitable_for_topic, published_at,
                evidence_text, rationale, review_method, reviewed_at
             FROM scholar_source_expertise_reviews
             WHERE finding_id = ?1 ORDER BY id",
            )
            .map_err(database_error)?;
        statement
            .query_map([finding_id], |row| {
                Ok(ScholarSourceExpertiseReview {
                    id: row.get(0)?,
                    finding_id: row.get(1)?,
                    source_kind: row.get(2)?,
                    suitable_for_topic: row.get::<_, i64>(3)? != 0,
                    published_at: row.get(4)?,
                    evidence_text: row.get(5)?,
                    rationale: row.get(6)?,
                    review_method: row.get(7)?,
                    reviewed_at: row.get(8)?,
                })
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)
    }

    pub fn review_scholar_source_origin(
        &mut self,
        finding_id: i64,
        underlying_source_id: &str,
        rationale: &str,
    ) -> io::Result<()> {
        let identity = underlying_source_id.trim();
        if !crate::storage::valid_bounded_field(identity, 200) {
            return Err(invalid(
                "underlying source identity must contain 1 to 200 printable characters",
            ));
        }
        require_printable_field(
            rationale,
            500,
            "source origin review reason must contain 1 to 500 printable characters",
        )?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let finding: SourceOriginFinding = tx
            .query_row(
                "SELECT f.status, f.source_excerpt, f.evidence_text, a.source_kind,
                    f.subject, f.relation, f.object,
                    f.valid_from, f.valid_until, f.geographic_scope, f.context_scope
                 FROM scholar_findings f
                 JOIN scholar_source_assessments a ON a.finding_id = f.id
                 WHERE f.id = ?1",
                [finding_id],
                |row| {
                    Ok(SourceOriginFinding {
                        evidence: FindingEvidence::from_row(row)?,
                        source_kind: row.get(3)?,
                        proposition: super::ScholarPropositionScope::from_row(row, 4)?,
                    })
                },
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("no assessed Scholar finding with that ID"))?;
        let SourceOriginFinding {
            evidence,
            source_kind,
            proposition,
        } = finding;
        evidence.require_pending_review(
            "source origin can only be reviewed for a finding awaiting review",
        )?;
        if source_kind != "secondary" {
            return Err(invalid(
                "underlying publication identities apply to secondary sources",
            ));
        }
        evidence.require_evidence_quote()?;
        let latest_topic_review: Option<bool> = tx
            .query_row(
                "SELECT suitable_for_topic FROM scholar_source_expertise_reviews
                 WHERE finding_id = ?1 ORDER BY id DESC LIMIT 1",
                [finding_id],
                |row| row.get::<_, i64>(0).map(|value| value != 0),
            )
            .optional()
            .map_err(database_error)?;
        if latest_topic_review != Some(true) {
            return Err(invalid(
                "review source suitability for this topic before verifying its origin",
            ));
        }
        let normalized_identity = canonical(identity);
        tx.execute(
            "INSERT INTO scholar_source_origin_reviews
                (finding_id, underlying_source_id, evidence_text, rationale, reviewed_at)
             VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                finding_id,
                normalized_identity,
                evidence.evidence_text,
                rationale
            ],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_source_assessments SET gate_status = 'awaiting_corroboration',
                reason = 'underlying publication identity changed; recomputing independent corroboration'
             WHERE gate_status = 'eligible'
               AND reason LIKE '%distinct verified underlying publication identities%'
               AND finding_id IN (
                   SELECT id FROM scholar_findings WHERE lower(trim(subject)) = lower(trim(?1))
                     AND lower(trim(relation)) = lower(trim(?2))
                     AND lower(trim(object)) = lower(trim(?3))
                     AND valid_from IS ?4 AND valid_until IS ?5
                     AND geographic_scope IS ?6 AND context_scope IS ?7
               )",
            params![proposition.subject, proposition.relation, proposition.proposition.object,
                proposition.proposition.valid_from, proposition.proposition.valid_until,
                proposition.proposition.geographic_scope, proposition.proposition.context_scope],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_source_assessments SET independence_key = ?2,
                independence_verified = 1,
                reason = substr(reason, 1, 1000) || '; reviewer verified underlying publication identity: ' || ?2 || '. Origin review: ' || ?3
             WHERE finding_id = ?1",
            params![finding_id, normalized_identity, rationale],
        )
        .map_err(database_error)?;

        let qualifying: Vec<i64> = {
            let query = format!(
                    "WITH {AUTOMATIC_PROMOTION_ELIGIBILITY_CTE}
                     SELECT f.id FROM eligible_findings f
                     WHERE lower(trim(f.subject)) = lower(trim(?1))
                       AND lower(trim(f.relation)) = lower(trim(?2))
                       AND lower(trim(f.object)) = lower(trim(?3))
                       AND f.valid_from IS ?4 AND f.valid_until IS ?5
                       AND f.geographic_scope IS ?6 AND f.context_scope IS ?7
                       AND EXISTS (
                         SELECT 1 FROM eligible_findings other
                         WHERE lower(trim(other.subject)) = lower(trim(f.subject))
                           AND lower(trim(other.relation)) = lower(trim(f.relation))
                           AND lower(trim(other.object)) = lower(trim(f.object))
                           AND other.valid_from IS f.valid_from AND other.valid_until IS f.valid_until
                           AND other.geographic_scope IS f.geographic_scope
                           AND other.context_scope IS f.context_scope
                           AND other.id <> f.id
                           AND other.independence_key <> f.independence_key
                       AND NOT EXISTS (
                         SELECT 1 FROM scholar_provenance_reviews pr
                         WHERE ((pr.finding_id = f.id AND pr.related_finding_id = other.id)
                            OR (pr.finding_id = other.id AND pr.related_finding_id = f.id))
                           AND (pr.status <> 'reviewed' OR pr.verdict <> 'possibly_distinct_reporting')
                       )
                       )
                     ORDER BY f.id",
                );
            let mut statement = tx.prepare(&query).map_err(database_error)?;
            statement
                .query_map(
                    params![
                        proposition.subject,
                        proposition.relation,
                        proposition.proposition.object,
                        proposition.proposition.valid_from,
                        proposition.proposition.valid_until,
                        proposition.proposition.geographic_scope,
                        proposition.proposition.context_scope,
                        automatic_promotion_confidence(),
                    ],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?
        };
        for id in qualifying {
            tx.execute(
                "UPDATE scholar_source_assessments SET gate_status = 'eligible',
                    reason = 'two directly supporting secondary sources have distinct origins; confidence meets the configured automatic acceptance threshold'
                 WHERE finding_id = ?1",
                [id],
            )
            .map_err(database_error)?;
        }
        self_promote_verified_findings(&tx, &proposition)?;
        tx.commit().map_err(database_error)
    }

    pub fn scholar_source_origin_reviews(
        &self,
        finding_id: i64,
    ) -> io::Result<Vec<ScholarSourceOriginReview>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, finding_id, underlying_source_id, evidence_text, rationale, review_method, reviewed_at
             FROM scholar_source_origin_reviews WHERE finding_id = ?1 ORDER BY id",
            )
            .map_err(database_error)?;
        statement
            .query_map([finding_id], |row| {
                Ok(ScholarSourceOriginReview {
                    id: row.get(0)?,
                    finding_id: row.get(1)?,
                    underlying_source_id: row.get(2)?,
                    evidence_text: row.get(3)?,
                    rationale: row.get(4)?,
                    review_method: row.get(5)?,
                    reviewed_at: row.get(6)?,
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
        self.review_scholar_finding_with_method(finding_id, accept, reason, "human", None)
    }

    pub fn review_scholar_finding_ai(
        &mut self,
        finding_id: i64,
        accept: bool,
        reason: &str,
    ) -> io::Result<()> {
        self.review_scholar_finding_with_method(finding_id, accept, reason, "ai", None)
    }

    #[cfg(test)]
    pub fn reject_scholar_finding_with_feedback(
        &mut self,
        finding_id: i64,
        reason: &str,
        route: &str,
        guidance: &str,
    ) -> io::Result<()> {
        self.review_scholar_finding_with_method(
            finding_id,
            false,
            reason,
            "ai",
            Some((route, guidance, None, None)),
        )
    }

    pub fn reject_scholar_finding_with_source_feedback(
        &mut self,
        finding_id: i64,
        reason: &str,
        route: &str,
        guidance: &str,
        exclusion_reason: Option<&str>,
        suggested_evidence: Option<&str>,
    ) -> io::Result<()> {
        self.review_scholar_finding_with_method(
            finding_id,
            false,
            reason,
            "ai",
            Some((route, guidance, exclusion_reason, suggested_evidence)),
        )
    }

    fn review_scholar_finding_with_method(
        &mut self,
        finding_id: i64,
        accept: bool,
        reason: &str,
        method: &str,
        feedback: Option<(&str, &str, Option<&str>, Option<&str>)>,
    ) -> io::Result<()> {
        validate_review_reason(reason)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let finding = tx
            .query_row(
                "SELECT status, source_excerpt, evidence_text, job_id, source_url,
                    subject, relation, object, valid_from, valid_until, attribution,
                    polarity, modality, geographic_scope, context_scope, source_title
             FROM scholar_findings WHERE id = ?1",
                [finding_id],
                ScholarFindingReview::from_row,
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("no scholar finding with that ID"))?;
        let ScholarFindingReview {
            job_id,
            source_url,
            source_title,
            evidence,
            subject,
            relation,
            object,
            valid_from,
            valid_until,
            proposition,
        } = finding;
        evidence.require_pending_review("scholar finding is already resolved")?;
        if accept {
            let evidence_quote_present = evidence.contains_quote(&evidence.evidence_text);
            let evidence_finding = ScholarFinding {
                semantic_support: None,
                subject: subject.clone(),
                relation: relation.clone(),
                object: object.clone(),
                evidence_text: evidence.evidence_text,
                confidence: 0.0,
                valid_from: valid_from.clone(),
                valid_until: valid_until.clone(),
                source_quality: "unknown".into(),
                source_updated_at: None,
                proposition: proposition.clone(),
            };
            if matches!(canonical(&subject).as_str(), "user" | "the user") {
                return Err(invalid(
                    "web evidence cannot establish a personal fact about the user",
                ));
            }
            let saved_support = super::saved_semantic_support(&tx, finding_id)?;
            if let Some(saved) = saved_support {
                let support: crate::semantic::SemanticSupport =
                    serde_json::from_str(&saved).map_err(invalid_json)?;
                if !support.supports(&evidence_finding, &evidence.source_excerpt) {
                    return Err(invalid(
                        "saved semantic assessment no longer supports this finding",
                    ));
                }
            }
            let direct_support: Option<i64> = tx
                .query_row(
                    "SELECT direct_support FROM scholar_source_assessments WHERE finding_id = ?1",
                    [finding_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(database_error)?
                .flatten();
            if direct_support == Some(0) {
                return Err(invalid(
                    "cannot accept a finding without direct passage support",
                ));
            }
            if direct_support.is_none() {
                // Legacy findings were backfilled as unassessed. Re-check their
                // excerpt before a reviewer can promote them.
                let supported = evidence_quote_present
                    && crate::storage::direct_passage_supports_finding(
                        &source_title,
                        &evidence_finding,
                    );
                if !supported {
                    return Err(invalid("legacy finding lacks direct passage support"));
                }
            }
        }
        let semantic_id = if accept {
            let mut statement = tx
                .prepare(
                    "SELECT subject, relation, id, object, valid_from, valid_until, geographic_scope, context_scope
                 FROM semantic_claims WHERE retracted_by IS NULL",
                )
                .map_err(database_error)?;
            let current = statement
                .query_map([], |row| {
                    Ok(ExistingSemanticClaim {
                        subject: row.get(0)?,
                        relation: row.get(1)?,
                        scope: super::SemanticClaimScope::from_row(row, 2)?,
                    })
                })
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?;
            drop(statement);
            let today = crate::storage::current_sql_date(&tx)?;
            let aliases = super::entities::EntityAliases::load(&tx)?;
            let equivalent = current
                .iter()
                .find(|claim| {
                    aliases.same(&claim.subject, &subject)
                        && canonical(&claim.relation) == canonical(&relation)
                        && canonical(&claim.scope.proposition.object) == canonical(&object)
                        && claim.scope.proposition.valid_from.as_deref() == valid_from.as_deref()
                        && claim.scope.proposition.valid_until.as_deref() == valid_until.as_deref()
                        && claim
                            .scope
                            .proposition
                            .same_scope(&proposition.geographic_scope, &proposition.context_scope)
                })
                .map(|claim| claim.scope.id);
            if let Some(id) = equivalent {
                Some(id)
            } else {
                let conflicting = current
                    .iter()
                    .find(|claim| {
                        aliases.same(&claim.subject, &subject)
                            && canonical(&claim.relation) == canonical(&relation)
                            && canonical(&claim.scope.proposition.object) != canonical(&object)
                            && claim.scope.proposition.same_scope(
                                &proposition.geographic_scope,
                                &proposition.context_scope,
                            )
                            && claim_periods_overlap(
                                valid_from.as_deref(),
                                valid_until.as_deref(),
                                claim.scope.proposition.valid_from.as_deref(),
                                claim.scope.proposition.valid_until.as_deref(),
                                &today,
                            )
                    })
                    .map(|claim| {
                        (
                            claim.scope.id,
                            claim.scope.proposition.valid_from.clone(),
                            claim.scope.proposition.valid_until.clone(),
                        )
                    });
                let supersedes_id = if let Some((id, old_from, old_until)) = &conflicting {
                    if old_from.as_deref() == valid_from.as_deref()
                        && old_until.as_deref() == valid_until.as_deref()
                    {
                        super::supersede_semantic_claim(
                            &tx,
                            *id,
                            &format!(
                                "superseded by reviewed scholar finding #{finding_id}: {}",
                                reason.trim()
                            ),
                        )?;
                        Some(*id)
                    } else {
                        // A partial time overlap does not invalidate the old claim's
                        // entire period. Keep it active but challenged for review.
                        super::mark_semantic_claim_challenged(&tx, *id)?;
                        None
                    }
                } else {
                    None
                };
                Some(super::insert_scholar_semantic_claim(
                    &tx,
                    super::NewSemanticClaim {
                        subject: &subject,
                        relation: &relation,
                        object: &object,
                        source: &source_url,
                        supersedes_id,
                        valid_from: valid_from.as_deref(),
                        valid_until: valid_until.as_deref(),
                        proposition: &proposition,
                    },
                )?)
            }
        } else {
            None
        };
        tx.execute(
            "UPDATE scholar_findings SET status = ?2, semantic_claim_id = ?3, decision_reason = ?4 WHERE id = ?1",
            params![finding_id, if accept { "promoted" } else { "rejected" }, semantic_id, reason.trim()],
        ).map_err(database_error)?;
        let proposal_id = super::latest_scholar_finding_proposal_id(&tx, finding_id)?;
        record_policy_decision(
            &tx,
            proposal_id,
            review_decision_outcome(method, accept),
            reason.trim(),
            semantic_id,
            Some(if accept { "active" } else { "rejected" }),
        )?;
        tx.execute(
            "UPDATE scholar_jobs SET status = ?2, last_error = NULL WHERE id = ?1",
            params![job_id, if accept { "promoted" } else { "no_result" }],
        )
        .map_err(database_error)?;
        super::set_scholar_task_stop_reason(
            &tx,
            &job_id,
            if accept {
                "complete"
            } else {
                "reviewer_rejected"
            },
        )?;
        if let Some(claim_id) = semantic_id {
            link_claim_evidence(
                &tx,
                claim_id,
                "web",
                Some(&source_url),
                None,
                &evidence.source_excerpt,
                "supports",
            )?;
            super::activate_reviewed_claim(&tx, claim_id)?;
        }
        if let Some((route, guidance, exclusion_reason, suggested_evidence)) = feedback {
            feedback::queue_feedback(
                &tx,
                finding_id,
                route,
                guidance,
                &format!("review:{finding_id}"),
                exclusion_reason,
                suggested_evidence,
            )?;
        } else {
            tx.execute("UPDATE scholar_feedback_sessions SET status = ?2, last_reason = ?3 WHERE root_finding_id = (SELECT root_finding_id FROM scholar_research_feedback WHERE scholar_job_id = ?1)", params![job_id, if accept { "accepted" } else { "exhausted" }, reason]).map_err(database_error)?;
        }
        tx.commit().map_err(database_error)?;
        if feedback.is_some() {
            self.log_scholar_feedback(finding_id)?;
        }
        Ok(())
    }
}
