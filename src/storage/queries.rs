use super::*;

fn research_relevance_weight(relevance: Option<&str>) -> usize {
    match relevance {
        Some("direct") => 100,
        Some("related") => 75,
        Some("tangential") => 40,
        _ => 100,
    }
}

impl LearningStore {
    pub fn claim_count(&self) -> io::Result<usize> {
        let count: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM semantic_claims", [], |row| row.get(0))
            .map_err(database_error)?;
        usize::try_from(count).map_err(|_| invalid("SQLite returned a negative claim count"))
    }

    pub fn relevant_semantic_claims(
        &self,
        query: &str,
        limit: usize,
    ) -> io::Result<Vec<SemanticClaim>> {
        let query_words = relevant_words(query);
        if query_words.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, subject, relation, object, source, source_observation_id,
                    status, volatility, valid_from, valid_until, observed_at, checked_at,
                    last_reviewed_at, next_review_at,
                    COALESCE((SELECT confidence FROM scholar_findings f WHERE f.semantic_claim_id = semantic_claims.id ORDER BY f.id DESC LIMIT 1),
                             (SELECT confidence FROM candidate_claims c WHERE c.semantic_claim_id = semantic_claims.id ORDER BY rowid DESC LIMIT 1)),
                    COALESCE((SELECT source_quality FROM scholar_findings f WHERE f.semantic_claim_id = semantic_claims.id ORDER BY f.id DESC LIMIT 1),
                             CASE WHEN source_observation_id IS NOT NULL THEN 'user_reported' END),
                    COALESCE((SELECT freshness FROM scholar_findings f WHERE f.semantic_claim_id = semantic_claims.id ORDER BY f.id DESC LIMIT 1),
                             CASE WHEN source_observation_id IS NOT NULL THEN 'not_applicable' END),
                    (SELECT a.research_relevance FROM scholar_findings f
                     JOIN scholar_source_assessments a ON a.finding_id = f.id
                     WHERE f.semantic_claim_id = semantic_claims.id ORDER BY f.id DESC LIMIT 1)
             FROM semantic_claims WHERE retracted_by IS NULL ORDER BY id DESC LIMIT 1000",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(SemanticClaim {
                    id: row.get(0)?,
                    subject: row.get(1)?,
                    relation: row.get(2)?,
                    object: row.get(3)?,
                    source: row.get(4)?,
                    source_observation_id: row.get(5)?,
                    status: row.get(6)?,
                    volatility: row.get(7)?,
                    valid_from: row.get(8)?,
                    valid_until: row.get(9)?,
                    observed_at: row.get(10)?,
                    checked_at: row.get(11)?,
                    last_reviewed_at: row.get(12)?,
                    next_review_at: row.get(13)?,
                    extraction_confidence: row.get(14)?,
                    source_quality: row.get(15)?,
                    freshness: row.get(16)?,
                    research_relevance: row.get(17)?,
                })
            })
            .map_err(database_error)?;
        let mut ranked = Vec::new();
        for row in rows {
            let claim = row.map_err(database_error)?;
            let words = relevant_words(&format!(
                "{} {} {}",
                claim.subject, claim.relation, claim.object
            ));
            let score = lexical_overlap(&words, &query_words)
                * research_relevance_weight(claim.research_relevance.as_deref());
            if score > 0 {
                ranked.push((score, claim));
            }
        }
        ranked.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(ranked
            .into_iter()
            .take(limit)
            .map(|(_, claim)| claim)
            .collect())
    }

    pub fn relevant_scholar_evidence(
        &self,
        query: &str,
        limit: usize,
    ) -> io::Result<Vec<ScholarEvidenceForAnswer>> {
        let query_words = relevant_words(query);
        if query_words.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let rows = self.scholar_evidence_candidates(false)?;
        let mut ranked = Vec::new();
        for row in rows {
            let finding = row;
            let words = relevant_words(&format!(
                "{} {} {} {}",
                finding.subject, finding.relation, finding.object, finding.evidence_text
            ));
            let score = lexical_overlap(&words, &query_words)
                * research_relevance_weight(Some(&finding.research_relevance));
            if score > 0 {
                ranked.push((score, finding));
            }
        }
        ranked.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(ranked
            .into_iter()
            .take(limit)
            .map(|(_, finding)| finding)
            .collect())
    }

    pub fn scholar_evidence_candidates(
        &self,
        include_promoted: bool,
    ) -> io::Result<Vec<ScholarEvidenceForAnswer>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT f.id, f.source_archive_id, f.evidence_byte_start, f.evidence_byte_end,
                    f.source_url, f.subject, f.relation, f.object,
                    f.attribution, f.polarity, f.modality, f.geographic_scope, f.context_scope,
                    f.evidence_text, f.confidence, f.status, a.gate_status, a.reason,
                    a.research_relevance, a.topic_class, a.source_kind, f.freshness,
                    a.currency, a.direct_support, a.independence_key,
                    a.independence_verified, a.published_at,
                    COALESCE((SELECT json_group_array(subject || ' ' || relation || ' ' || object ||
                        ' [valid ' || COALESCE(valid_from, 'unknown start') || ' to ' ||
                        COALESCE(valid_until, 'unknown end') || '; geography ' || COALESCE(geographic_scope, 'unknown') ||
                        '; context ' || COALESCE(context_scope, 'unknown') || ']')
                      FROM semantic_claims c
                      WHERE c.retracted_by IS NULL AND c.status IN ('active','uncertain','challenged')
                        AND lower(trim(c.subject)) = lower(trim(f.subject))
                        AND lower(trim(c.relation)) = lower(trim(f.relation))
                        AND lower(trim(c.object)) != lower(trim(f.object))
                        AND EXISTS (SELECT 1 FROM scholar_finding_claim_links l
                                    WHERE l.finding_id = f.id AND l.claim_id = c.id
                                      AND l.relation = 'contradicts')), '[]')
                 FROM scholar_findings f
                 JOIN scholar_source_assessments a ON a.finding_id = f.id
                 WHERE (f.status = 'needs_review' OR (?1 AND f.status = 'promoted' AND EXISTS(
                       SELECT 1 FROM semantic_claims c WHERE c.id = f.semantic_claim_id
                         AND c.retracted_by IS NULL AND c.status IN ('active','uncertain'))))
                   AND a.direct_support = 1
                   AND instr(f.source_excerpt, f.evidence_text) > 0
                   AND lower(trim(f.subject)) NOT IN ('user', 'the user')
                 ORDER BY f.id DESC LIMIT 500",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([include_promoted], |row| {
                Ok(ScholarEvidenceForAnswer {
                    id: row.get(0)?,
                    source_archive_id: row.get(1)?,
                    evidence_byte_start: row.get(2)?,
                    evidence_byte_end: row.get(3)?,
                    source_url: row.get(4)?,
                    subject: row.get(5)?,
                    relation: row.get(6)?,
                    object: row.get(7)?,
                    attribution: row.get(8)?,
                    polarity: row.get(9)?,
                    modality: row.get(10)?,
                    geographic_scope: row.get(11)?,
                    context_scope: row.get(12)?,
                    evidence_text: row.get(13)?,
                    confidence: row.get(14)?,
                    finding_status: row.get(15)?,
                    gate_status: row.get(16)?,
                    reason: row.get(17)?,
                    research_relevance: row.get(18)?,
                    topic_class: row.get(19)?,
                    source_kind: row.get(20)?,
                    freshness: row.get(21)?,
                    currency: row.get(22)?,
                    direct_support: row.get::<_, Option<bool>>(23)?.unwrap_or(false),
                    independence_key: row.get(24)?,
                    independence_verified: row.get(25)?,
                    published_at: row.get(26)?,
                    conflicting_claims: serde_json::from_str(&row.get::<_, String>(27)?)
                        .unwrap_or_default(),
                })
            })
            .map_err(database_error)?;
        rows.map(|row| row.map_err(database_error)).collect()
    }

    pub fn inbox_summary(&self) -> io::Result<InboxSummary> {
        let counts = ["pending", "processing", "needs_review"]
            .into_iter()
            .map(|status| {
                self.connection
                    .query_row(
                        "SELECT COUNT(*) FROM observations WHERE status = ?1",
                        [status],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(database_error)
                    .and_then(|count| {
                        usize::try_from(count).map_err(|_| invalid("negative inbox count"))
                    })
            })
            .collect::<io::Result<Vec<_>>>()?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, conversation_id, turn_number, raw_text, status, created_at
             FROM observations WHERE status IN ('pending', 'processing', 'needs_review')
             ORDER BY created_at, turn_number",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| {
                let turn: i64 = row.get(2)?;
                Ok(Observation {
                    id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    turn_number: usize::try_from(turn)
                        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(2, turn))?,
                    raw_text: row.get(3)?,
                    status: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })
            .map_err(database_error)?;
        let observations = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        Ok(InboxSummary {
            pending: counts[0],
            processing: counts[1],
            needs_review: counts[2],
            observations,
        })
    }

    pub fn observation_detail(&self, id: &str) -> io::Result<Option<ObservationDetail>> {
        let observation = self
            .connection
            .query_row(
                "SELECT id, conversation_id, turn_number, raw_text, status, created_at, last_error
             FROM observations WHERE id = ?1",
                [id],
                |row| {
                    let turn: i64 = row.get(2)?;
                    Ok((
                        Observation {
                            id: row.get(0)?,
                            conversation_id: row.get(1)?,
                            turn_number: usize::try_from(turn)
                                .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(2, turn))?,
                            raw_text: row.get(3)?,
                            status: row.get(4)?,
                            created_at: row.get(5)?,
                        },
                        row.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?;
        let Some((observation, last_error)) = observation else {
            return Ok(None);
        };
        let mut statement = self
            .connection
            .prepare(
            "SELECT id, observation_id, subject, relation, object, evidence_text, confidence, status,
                    decision_reason, semantic_claim_id, valid_from, valid_until
             FROM candidate_claims WHERE observation_id = ?1 ORDER BY rowid",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([id], |row| {
                Ok(StoredCandidate {
                    id: row.get(0)?,
                    observation_id: row.get(1)?,
                    subject: row.get(2)?,
                    relation: row.get(3)?,
                    object: row.get(4)?,
                    evidence_text: row.get(5)?,
                    confidence: row.get(6)?,
                    status: row.get(7)?,
                    decision_reason: row.get(8)?,
                    semantic_claim_id: row.get(9)?,
                    valid_from: row.get(10)?,
                    valid_until: row.get(11)?,
                })
            })
            .map_err(database_error)?;
        let candidates = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        Ok(Some(ObservationDetail {
            observation,
            last_error,
            candidates,
        }))
    }

    pub fn observation_text(&self, id: &str) -> io::Result<Option<String>> {
        self.connection
            .query_row(
                "SELECT raw_text FROM observations WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)
    }

    pub fn relevant_review_candidates(
        &self,
        conversation_id: &str,
        query: &str,
        limit: usize,
    ) -> io::Result<Vec<StoredCandidate>> {
        let query_words = relevant_words(query);
        if query_words.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT c.id, c.observation_id, c.subject, c.relation, c.object,
                    c.evidence_text, c.confidence, c.status, c.decision_reason,
                    c.semantic_claim_id, o.turn_number, c.valid_from, c.valid_until
             FROM candidate_claims c JOIN observations o ON o.id = c.observation_id
             WHERE o.conversation_id = ?1 AND c.status IN ('proposed', 'needs_review')
             ORDER BY o.turn_number DESC LIMIT 256",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([conversation_id], |row| {
                Ok((
                    StoredCandidate {
                        id: row.get(0)?,
                        observation_id: row.get(1)?,
                        subject: row.get(2)?,
                        relation: row.get(3)?,
                        object: row.get(4)?,
                        evidence_text: row.get(5)?,
                        confidence: row.get(6)?,
                        status: row.get(7)?,
                        decision_reason: row.get(8)?,
                        semantic_claim_id: row.get(9)?,
                        valid_from: row.get(11)?,
                        valid_until: row.get(12)?,
                    },
                    row.get::<_, i64>(10)?,
                ))
            })
            .map_err(database_error)?;
        let mut ranked = Vec::new();
        for row in rows {
            let (candidate, turn) = row.map_err(database_error)?;
            let words = relevant_words(&format!(
                "{} {} {}",
                candidate.subject, candidate.relation, candidate.object
            ));
            let score = lexical_overlap(&words, &query_words);
            if score > 0 {
                ranked.push((score, turn, candidate));
            }
        }
        ranked.sort_by(|(left_score, left_turn, _), (right_score, right_turn, _)| {
            right_score
                .cmp(left_score)
                .then_with(|| right_turn.cmp(left_turn))
        });
        Ok(ranked
            .into_iter()
            .take(limit)
            .map(|(_, _, candidate)| candidate)
            .collect())
    }

    pub fn active_semantic_claims(&self) -> io::Result<Vec<SemanticClaimDetail>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, subject, relation, object, source, source_observation_id,
                    status, volatility, valid_from, valid_until, observed_at, checked_at,
                    last_reviewed_at, next_review_at,
                    COALESCE((SELECT confidence FROM scholar_findings f WHERE f.semantic_claim_id = semantic_claims.id ORDER BY f.id DESC LIMIT 1),
                             (SELECT confidence FROM candidate_claims c WHERE c.semantic_claim_id = semantic_claims.id ORDER BY rowid DESC LIMIT 1)),
                    COALESCE((SELECT source_quality FROM scholar_findings f WHERE f.semantic_claim_id = semantic_claims.id ORDER BY f.id DESC LIMIT 1),
                             CASE WHEN source_observation_id IS NOT NULL THEN 'user_reported' END),
                    COALESCE((SELECT freshness FROM scholar_findings f WHERE f.semantic_claim_id = semantic_claims.id ORDER BY f.id DESC LIMIT 1),
                             CASE WHEN source_observation_id IS NOT NULL THEN 'not_applicable' END)
             FROM semantic_claims WHERE retracted_by IS NULL ORDER BY id",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(SemanticClaim {
                    id: row.get(0)?,
                    subject: row.get(1)?,
                    relation: row.get(2)?,
                    object: row.get(3)?,
                    source: row.get(4)?,
                    source_observation_id: row.get(5)?,
                    status: row.get(6)?,
                    volatility: row.get(7)?,
                    valid_from: row.get(8)?,
                    valid_until: row.get(9)?,
                    observed_at: row.get(10)?,
                    checked_at: row.get(11)?,
                    last_reviewed_at: row.get(12)?,
                    next_review_at: row.get(13)?,
                    extraction_confidence: row.get(14)?,
                    source_quality: row.get(15)?,
                    freshness: row.get(16)?,
                    research_relevance: None,
                })
            })
            .map_err(database_error)?;
        let claims = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        drop(statement);
        claims
            .into_iter()
            .map(|claim| {
                let mut sources_statement = self
                    .connection
                    .prepare(
                        "SELECT o.id || ': ' || o.raw_text FROM semantic_claim_sources s
                 JOIN observations o ON o.id = s.observation_id
                 WHERE s.semantic_claim_id = ?1 ORDER BY o.created_at, o.turn_number",
                    )
                    .map_err(database_error)?;
                let sources = sources_statement
                    .query_map([claim.id], |row| row.get(0))
                    .map_err(database_error)?
                    .collect::<Result<Vec<String>, _>>()
                    .map_err(database_error)?;
                let mut sources: Vec<String> = sources;
                let mut scholar_statement = self
                    .connection
                    .prepare(
                        "SELECT source_url || ': ' || evidence_text FROM scholar_findings
                     WHERE semantic_claim_id = ?1 AND status = 'promoted' ORDER BY id",
                    )
                    .map_err(database_error)?;
                let scholar_sources = scholar_statement
                    .query_map([claim.id], |row| row.get(0))
                    .map_err(database_error)?
                    .collect::<Result<Vec<String>, _>>()
                    .map_err(database_error)?;
                sources.extend(scholar_sources);
                Ok(SemanticClaimDetail { claim, sources })
            })
            .collect()
    }
}
