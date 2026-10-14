//! Scholar source assessments, provenance decisions and finding review.
use super::*;

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
        if rationale.trim().is_empty()
            || rationale.chars().count() > 500
            || rationale.chars().any(char::is_control)
        {
            return Err(invalid(
                "source review reason must contain 1 to 500 printable characters",
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let finding: (String, String, f64, String) = tx
            .query_row(
                "SELECT evidence_text, status, confidence, source_excerpt
                 FROM scholar_findings WHERE id = ?1",
                [finding_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("no scholar finding with that ID"))?;
        let (evidence_text, finding_status, confidence, source_excerpt) = finding;
        if finding_status != "needs_review" {
            return Err(invalid(
                "source expertise can only be reviewed for a finding awaiting review",
            ));
        }
        if !source_excerpt.contains(&evidence_text) {
            return Err(invalid(
                "finding evidence is not present in its stored source passage",
            ));
        }
        let published_at = if let Some(date) = published_at {
            let normalized: Option<String> = tx
                .query_row("SELECT date(?1)", [date], |row| row.get(0))
                .map_err(database_error)?;
            Some(normalized.ok_or_else(|| invalid("publication date must be a valid ISO date"))?)
        } else {
            None
        };
        let assessment: (String, Option<i64>, bool, String) = tx
            .query_row(
                "SELECT topic_class, direct_support, independence_verified, gate_status
                 FROM scholar_source_assessments WHERE finding_id = ?1",
                [finding_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get::<_, i64>(2)? != 0,
                        row.get(3)?,
                    ))
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
                evidence_text,
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
        let is_recent = match published_at.as_deref() {
            Some(date) => tx
                .query_row(
                    "SELECT COALESCE(date(?1) > date('now', '-12 months') AND date(?1) <= date('now'), 0)",
                    [date],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(database_error)?,
            None => false,
        };
        let (gate_status, topical_expertise, gate_reason) = if !suitable_for_topic {
            (
                "pending_review",
                "reviewer_found_topic_mismatch",
                "reviewer found the source unsuitable for this claim's topic",
            )
        } else if assessment.1 != Some(1) {
            (
                "unsupported",
                "reviewer_verified_for_claim",
                "source suitability was reviewed, but the stored passage does not directly support this claim",
            )
        } else if confidence < PROMOTION_CONFIDENCE {
            (
                "pending_review",
                "reviewer_verified_for_claim",
                "source suitability was reviewed, but extraction confidence remains below the threshold",
            )
        } else if assessment.0 == "sensitive" {
            (
                "pending_review",
                "reviewer_verified_for_claim",
                "sensitive claims still require explicit finding review",
            )
        } else if assessment.0 == "recent" && !is_recent {
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
        } else if assessment.2 && assessment.3 == "eligible" {
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
        if identity.is_empty()
            || identity.chars().count() > 200
            || identity.chars().any(char::is_control)
            || identity.contains("<|")
        {
            return Err(invalid(
                "underlying source identity must contain 1 to 200 printable characters",
            ));
        }
        if rationale.trim().is_empty()
            || rationale.chars().count() > 500
            || rationale.chars().any(char::is_control)
        {
            return Err(invalid(
                "source origin review reason must contain 1 to 500 printable characters",
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let finding: (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = tx
            .query_row(
                "SELECT f.status, f.source_excerpt, f.evidence_text, a.source_kind,
                    f.subject, f.relation, f.object,
                    f.valid_from, f.valid_until, f.geographic_scope, f.context_scope
                 FROM scholar_findings f
                 JOIN scholar_source_assessments a ON a.finding_id = f.id
                 WHERE f.id = ?1",
                [finding_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| invalid("no assessed Scholar finding with that ID"))?;
        let (
            status,
            source_excerpt,
            evidence_text,
            source_kind,
            subject,
            relation,
            object,
            valid_from,
            valid_until,
            geographic_scope,
            context_scope,
        ) = finding;
        if status != "needs_review" {
            return Err(invalid(
                "source origin can only be reviewed for a finding awaiting review",
            ));
        }
        if source_kind != "secondary" {
            return Err(invalid(
                "underlying publication identities apply to secondary sources",
            ));
        }
        if !source_excerpt.contains(&evidence_text) {
            return Err(invalid(
                "finding evidence is not present in its stored source passage",
            ));
        }
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
            params![finding_id, normalized_identity, evidence_text, rationale],
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
            params![subject, relation, object, valid_from, valid_until, geographic_scope, context_scope],
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
            let mut statement = tx
                .prepare(
                    "SELECT f.id FROM scholar_findings f
                     JOIN scholar_source_assessments a ON a.finding_id = f.id
                     WHERE lower(trim(f.subject)) = lower(trim(?1))
                       AND lower(trim(f.relation)) = lower(trim(?2))
                       AND lower(trim(f.object)) = lower(trim(?3))
                       AND f.valid_from IS ?4 AND f.valid_until IS ?5
                       AND f.geographic_scope IS ?6 AND f.context_scope IS ?7
                       AND f.status IN ('needs_review', 'promoted')
                       AND f.confidence >= ?8
                       AND a.source_kind = 'secondary'
                       AND a.direct_support = 1 AND a.independence_verified = 1
                       AND (SELECT suitable_for_topic FROM scholar_source_expertise_reviews er
                            WHERE er.finding_id = f.id ORDER BY er.id DESC LIMIT 1) = 1
                       AND EXISTS (
                         SELECT 1 FROM scholar_findings other
                         JOIN scholar_source_assessments other_a ON other_a.finding_id = other.id
                         WHERE lower(trim(other.subject)) = lower(trim(f.subject))
                           AND lower(trim(other.relation)) = lower(trim(f.relation))
                           AND lower(trim(other.object)) = lower(trim(f.object))
                           AND other.valid_from IS f.valid_from AND other.valid_until IS f.valid_until
                           AND other.geographic_scope IS f.geographic_scope
                           AND other.context_scope IS f.context_scope
                           AND other.id <> f.id AND other.status IN ('needs_review', 'promoted')
                           AND other.confidence >= ?8
                           AND other_a.source_kind = 'secondary'
                           AND other_a.direct_support = 1 AND other_a.independence_verified = 1
                           AND other_a.independence_key <> a.independence_key
                       AND (SELECT suitable_for_topic FROM scholar_source_expertise_reviews other_er
                            WHERE other_er.finding_id = other.id ORDER BY other_er.id DESC LIMIT 1) = 1
                       AND NOT EXISTS (
                         SELECT 1 FROM scholar_provenance_reviews pr
                         WHERE ((pr.finding_id = f.id AND pr.related_finding_id = other.id)
                            OR (pr.finding_id = other.id AND pr.related_finding_id = f.id))
                           AND (pr.status <> 'reviewed' OR pr.verdict <> 'possibly_distinct_reporting')
                       )
                       )
                     ORDER BY f.id",
                )
                .map_err(database_error)?;
            statement
                .query_map(
                    params![
                        subject,
                        relation,
                        object,
                        valid_from,
                        valid_until,
                        geographic_scope,
                        context_scope,
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
        self_promote_verified_findings(
            &tx,
            &subject,
            &relation,
            &object,
            valid_from.as_deref(),
            valid_until.as_deref(),
            geographic_scope.as_deref(),
            context_scope.as_deref(),
        )?;
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
                    valid_from, valid_until, attribution, polarity, modality,
                    geographic_scope, context_scope
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
                        row.get::<_, Option<String>>(9)?,
                        row.get::<_, String>(10)?,
                        row.get::<_, String>(11)?,
                        row.get::<_, Option<String>>(12)?,
                        row.get::<_, Option<String>>(13)?,
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
            attribution,
            polarity,
            modality,
            geographic_scope,
            context_scope,
        ) = finding;
        if status != "needs_review" {
            return Err(invalid("scholar finding is already resolved"));
        }
        if accept {
            if matches!(canonical(&subject).as_str(), "user" | "the user") {
                return Err(invalid(
                    "web evidence cannot establish a personal fact about the user",
                ));
            }
            let saved_support: Option<String> = tx
                .query_row(
                    "SELECT assessment_json FROM scholar_semantic_support WHERE finding_id = ?1",
                    [finding_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(database_error)?;
            if let Some(saved) = saved_support {
                let support: crate::semantic::SemanticSupport =
                    serde_json::from_str(&saved).map_err(invalid_json)?;
                let evidence: String = tx
                    .query_row(
                        "SELECT evidence_text FROM scholar_findings WHERE id = ?1",
                        [finding_id],
                        |row| row.get(0),
                    )
                    .map_err(database_error)?;
                let bound = ScholarFinding {
                    semantic_support: None,
                    subject: subject.clone(),
                    relation: relation.clone(),
                    object: object.clone(),
                    evidence_text: evidence,
                    confidence: 0.0,
                    valid_from: valid_from.clone(),
                    valid_until: valid_until.clone(),
                    source_quality: "unknown".into(),
                    source_updated_at: None,
                    proposition: PropositionMetadata {
                        attribution: attribution.clone(),
                        geographic_scope: geographic_scope.clone(),
                        context_scope: context_scope.clone(),
                        polarity: match polarity.as_str() {
                            "asserted" => ClaimPolarity::Asserted,
                            "negated" => ClaimPolarity::Negated,
                            _ => ClaimPolarity::Unknown,
                        },
                        modality: match modality.as_str() {
                            "factual" => ClaimModality::Factual,
                            "possible" => ClaimModality::Possible,
                            "necessary" => ClaimModality::Necessary,
                            "conditional" => ClaimModality::Conditional,
                            _ => ClaimModality::Unknown,
                        },
                    },
                };
                if !support.supports(&bound, &source_excerpt) {
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
                let (source_title, evidence_text): (String, String) = tx
                    .query_row(
                        "SELECT source_title, evidence_text FROM scholar_findings WHERE id = ?1",
                        [finding_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(database_error)?;
                let legacy = ScholarFinding {
                    semantic_support: None,
                    subject: subject.clone(),
                    relation: relation.clone(),
                    object: object.clone(),
                    evidence_text: evidence_text.clone(),
                    confidence: 0.0,
                    valid_from: valid_from.clone(),
                    valid_until: valid_until.clone(),
                    source_quality: "unknown".into(),
                    source_updated_at: None,
                    proposition: PropositionMetadata {
                        attribution: attribution.clone(),
                        geographic_scope: geographic_scope.clone(),
                        context_scope: context_scope.clone(),
                        polarity: match polarity.as_str() {
                            "asserted" => ClaimPolarity::Asserted,
                            "negated" => ClaimPolarity::Negated,
                            _ => ClaimPolarity::Unknown,
                        },
                        modality: match modality.as_str() {
                            "factual" => ClaimModality::Factual,
                            "possible" => ClaimModality::Possible,
                            "necessary" => ClaimModality::Necessary,
                            "conditional" => ClaimModality::Conditional,
                            _ => ClaimModality::Unknown,
                        },
                    },
                };
                let supported = source_excerpt.contains(&evidence_text)
                    && (evidence_supports_claim(&legacy)
                        || (matches!(
                            relation.as_str(),
                            "has encyclopedia summary" | "has encyclopedia detail"
                        ) && canonical(&subject) == canonical(&source_title)
                            && object == evidence_text));
                if !supported {
                    return Err(invalid("legacy finding lacks direct passage support"));
                }
            }
        }
        let semantic_id = if accept {
            let mut statement = tx
                .prepare(
                    "SELECT id, subject, relation, object, valid_from, valid_until, geographic_scope, context_scope
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
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                    ))
                })
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?;
            drop(statement);
            let today: String = tx
                .query_row("SELECT date('now')", [], |row| row.get(0))
                .map_err(database_error)?;
            let aliases = super::entities::EntityAliases::load(&tx)?;
            let equivalent = current
                .iter()
                .find(|(_, s, r, o, from, until, geo, context)| {
                    aliases.same(s, &subject)
                        && canonical(r) == canonical(&relation)
                        && canonical(o) == canonical(&object)
                        && from.as_deref() == valid_from.as_deref()
                        && until.as_deref() == valid_until.as_deref()
                        && geo == &geographic_scope
                        && context == &context_scope
                })
                .map(|(id, _, _, _, _, _, _, _)| *id);
            if let Some(id) = equivalent {
                Some(id)
            } else {
                let conflicting = current
                    .iter()
                    .find(|(_, s, r, o, from, until, geo, context)| {
                        aliases.same(s, &subject)
                            && canonical(r) == canonical(&relation)
                            && canonical(o) != canonical(&object)
                            && geo == &geographic_scope
                            && context == &context_scope
                            && claim_periods_overlap(
                                valid_from.as_deref(),
                                valid_until.as_deref(),
                                from.as_deref(),
                                until.as_deref(),
                                &today,
                            )
                    })
                    .map(|(id, _, _, _, from, until, _, _)| (*id, from.clone(), until.clone()));
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
                        (subject, relation, object, source, supersedes_id, source_observation_id, created_at, valid_from, valid_until, attribution, polarity, modality, geographic_scope, context_scope)
                     VALUES (?1, ?2, ?3, ?4, ?5, NULL, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![subject, relation, object, source_url, supersedes_id, valid_from, valid_until,
                        attribution, polarity, modality, geographic_scope, context_scope],
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
                if method == "ai" {
                    "ai_accepted"
                } else {
                    "human_accepted"
                }
            } else {
                if method == "ai" {
                    "ai_rejected"
                } else {
                    "human_rejected"
                }
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
        tx.execute(
            "UPDATE scholar_task_targets SET stop_reason=?2,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE job_id=?1",
            params![job_id, if accept { "complete" } else { "reviewer_rejected" }],
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
