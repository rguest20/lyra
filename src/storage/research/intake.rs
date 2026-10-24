use super::*;

struct SemanticSupportFinding {
    finding: ScholarFinding,
    passage: String,
    title: String,
}

struct ExistingSemanticClaim {
    id: i64,
    subject: String,
    object: String,
    valid_from: Option<String>,
    valid_until: Option<String>,
    relation: String,
    proposition: PropositionMetadata,
}

impl ExistingSemanticClaim {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            subject: row.get(1)?,
            object: row.get(2)?,
            valid_from: row.get(3)?,
            valid_until: row.get(4)?,
            relation: row.get(10)?,
            proposition: PropositionMetadata::from_storage_row(row, 5)?,
        })
    }
}

enum ExistingSemanticMatch {
    Conflict(i64),
    Match(i64),
    Qualifying(i64),
    Unresolved(i64),
    None,
}

impl SemanticSupportFinding {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            finding: ScholarFinding {
                semantic_support: None,
                subject: row.get(0)?,
                relation: row.get(1)?,
                object: row.get(2)?,
                evidence_text: row.get(3)?,
                confidence: row.get(4)?,
                valid_from: row.get(5)?,
                valid_until: row.get(6)?,
                source_quality: row.get(7)?,
                source_updated_at: row.get(8)?,
                proposition: PropositionMetadata::from_storage_row(row, 11)?,
            },
            passage: row.get(9)?,
            title: row.get(10)?,
        })
    }
}

fn source_version_for_finding(
    tx: &Transaction<'_>,
    job_id: &str,
    source_title: &str,
    source_url: &str,
    source_excerpt: &str,
    evidence_text: &str,
    source_updated_at: Option<&str>,
) -> io::Result<(i64, Option<i64>, Option<i64>)> {
    let mut statement = tx
        .prepare(
            "SELECT id,body FROM research_sources
             WHERE url=?1 AND instr(body,?2)>0 ORDER BY id DESC LIMIT 10",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map(params![source_url, evidence_text], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(statement);
    for (source_id, body) in rows {
        if let Some((start, end)) =
            super::evidence_byte_offsets_i64(&body, 0, &body, evidence_text)?
        {
            return Ok((source_id, Some(start), Some(end)));
        }
    }

    let retained =
        super::prepare_research_source(source_excerpt, super::MAX_RETAINED_SOURCE_CHARS)?;
    let query: Option<String> = tx
        .query_row(
            "SELECT question FROM scholar_jobs WHERE id=?1",
            [job_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?;
    super::insert_research_source(
        tx,
        super::NewResearchSource {
            job_id,
            title: source_title,
            url: source_url,
            content_hash: &retained.content_hash,
            body: &retained.body,
            source_scope: "finding_source_excerpt",
            input_characters: retained.input_characters,
            retained_characters: retained.retained_characters,
            truncated: retained.truncated,
            provider: "unknown",
            query: query.as_deref(),
            published_at: source_updated_at,
            capture_status: super::research_source_capture_status(
                "finding_source_excerpt",
                retained.truncated,
            ),
        },
    )?;
    let source_id: i64 = tx
        .query_row(
            "SELECT id FROM research_sources WHERE url=?1 AND content_hash=?2",
            params![source_url, retained.content_hash],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    let span = super::evidence_byte_offsets_i64(&retained.body, 0, &retained.body, evidence_text)?;
    Ok((
        source_id,
        span.map(|(start, _)| start),
        span.map(|(_, end)| end),
    ))
}

fn scholar_source_freshness(
    tx: &Transaction<'_>,
    finding: &ScholarFinding,
    time_sensitive: bool,
) -> io::Result<&'static str> {
    if !time_sensitive {
        return Ok("not_time_sensitive");
    }
    let Some(updated_at) = finding.source_updated_at.as_deref() else {
        return Ok("unknown");
    };
    let age_days: Option<f64> = tx
        .query_row(
            "SELECT julianday('now') - julianday(?1)",
            [updated_at],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?
        .flatten();
    Ok(match age_days {
        Some(days) if (0.0..=365.0).contains(&days) => "fresh",
        Some(days) if days > 365.0 => "stale",
        _ => "unknown",
    })
}

fn grounded_scholar_finding(
    source_title: &str,
    source_excerpt: &str,
    finding: &ScholarFinding,
) -> bool {
    let safe_evidence = !finding.evidence_text.trim().is_empty()
        && finding.evidence_text.chars().count() <= 4_500
        && !finding
            .evidence_text
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
        && !finding.evidence_text.contains("<|");
    let valid_scope = [
        &finding.proposition.geographic_scope,
        &finding.proposition.context_scope,
    ]
    .into_iter()
    .flatten()
    .all(|scope| {
        crate::storage::valid_quoted_scope(scope, source_excerpt, &finding.evidence_text)
            && !scope.contains("<|")
    });
    let semantic_support = finding.semantic_support.as_ref();
    let semantic_valid =
        semantic_support.is_none_or(|support| support.supports(finding, source_excerpt));
    semantic_valid
        // A claim-shaped sentence embedded in cookie controls or a footer is
        // still page chrome, not evidence. Keep this defense at ingestion as
        // well as page selection so alternate callers cannot bypass it.
        && !crate::scholar::passages::is_site_chrome(source_excerpt)
        && valid_scope
        && safe_evidence
        && crate::storage::valid_proposition_evidence(
            &finding.subject,
            &finding.relation,
            &finding.object,
            &finding.evidence_text,
            source_excerpt,
        )
        && (semantic_support.is_some()
            || crate::storage::direct_passage_supports_finding(source_title, finding))
        && crate::storage::valid_confidence(finding.confidence)
        && claim_period_is_stated(
            finding.valid_from.as_deref(),
            finding.valid_until.as_deref(),
            source_excerpt,
        )
}

struct IngestedFinding {
    id: i64,
    status: &'static str,
    reason: Option<String>,
}

fn existing_semantic_match(
    tx: &Transaction<'_>,
    finding: &ScholarFinding,
    today: &str,
) -> io::Result<ExistingSemanticMatch> {
    // Only directly reviewed, unscoped identity links can establish a shared
    // subject here. Contextual aliases require a scope-aware claim model and
    // must never silently become global equivalences.
    let aliases = super::entities::EntityAliases::load(tx)?;
    let mut statement = tx
        .prepare(
            "SELECT id, subject, object, valid_from, valid_until, attribution, polarity, modality, geographic_scope, context_scope, relation FROM semantic_claims
             WHERE retracted_by IS NULL
               AND lower(trim(relation)) = lower(trim(?2))
               AND (lower(trim(subject)) = lower(trim(?1)) OR EXISTS (
                    SELECT 1 FROM entity_equivalences e
                    WHERE (e.left_name = lower(trim(subject)) AND e.right_name = lower(trim(?1)))
                       OR (e.right_name = lower(trim(subject)) AND e.left_name = lower(trim(?1))))
                    OR EXISTS (
                    SELECT 1 FROM entity_alias_links l
                    WHERE lower(trim(l.identity_scope)) = lower(trim(?3))
                      AND ((l.left_name = lower(trim(subject)) AND l.right_name = lower(trim(?1)))
                        OR (l.right_name = lower(trim(subject)) AND l.left_name = lower(trim(?1))))
                      AND (l.source_finding_id IS NULL OR EXISTS (
                        SELECT 1 FROM scholar_findings f WHERE f.id=l.source_finding_id
                          AND (f.status='needs_review' OR (f.status='promoted' AND EXISTS (
                            SELECT 1 FROM semantic_claims c WHERE c.id=f.semantic_claim_id
                              AND c.retracted_by IS NULL AND c.status IN ('active','uncertain'))))))))
             ORDER BY id DESC",
        )
        .map_err(database_error)?;
    let existing = statement
        .query_map(
            params![
                finding.subject,
                finding.relation,
                finding.proposition.context_scope
            ],
            ExistingSemanticClaim::from_row,
        )
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let mut matching_claim_id = None;
    let mut conflicting_claim_id = None;
    let mut qualifying_claim_id = None;
    let mut unresolved_claim_id = None;
    for claim in existing {
        let same_subject = aliases.same(&claim.subject, &finding.subject)
            || finding
                .proposition
                .context_scope
                .as_deref()
                .is_some_and(|scope| {
                    claim
                        .proposition
                        .context_scope
                        .as_deref()
                        .is_some_and(|claim_scope| canonical(claim_scope) == canonical(scope))
                        && aliases.same_in_scope(&claim.subject, &finding.subject, scope)
                });
        if !same_subject || canonical(&claim.relation) != canonical(&finding.relation) {
            continue;
        }
        if !claim_periods_overlap(
            finding.valid_from.as_deref(),
            finding.valid_until.as_deref(),
            claim.valid_from.as_deref(),
            claim.valid_until.as_deref(),
            today,
        ) {
            continue;
        }
        let object_matches = canonical(&claim.object) == canonical(&finding.object);
        let validity_matches = claim.valid_from.as_deref() == finding.valid_from.as_deref()
            && claim.valid_until.as_deref() == finding.valid_until.as_deref();
        let proposition_scope_matches = claim.proposition.attribution
            == finding.proposition.attribution
            && claim.proposition.polarity == finding.proposition.polarity
            && claim.proposition.modality == finding.proposition.modality;
        let geographic_scope_matches =
            claim.proposition.geographic_scope == finding.proposition.geographic_scope;
        let context_scope_matches =
            claim.proposition.context_scope == finding.proposition.context_scope;
        if !geographic_scope_matches || !context_scope_matches {
            unresolved_claim_id.get_or_insert(claim.id);
            continue;
        }
        if object_matches && validity_matches && proposition_scope_matches {
            matching_claim_id.get_or_insert(claim.id);
            continue;
        }
        if !object_matches
            && proposition_scope_matches
            && finding.relation != "has encyclopedia detail"
        {
            conflicting_claim_id.get_or_insert(claim.id);
            continue;
        }
        qualifying_claim_id.get_or_insert(claim.id);
    }
    if let Some(id) = conflicting_claim_id {
        Ok(ExistingSemanticMatch::Conflict(id))
    } else if let Some(id) = matching_claim_id {
        Ok(ExistingSemanticMatch::Match(id))
    } else if let Some(id) = qualifying_claim_id {
        Ok(ExistingSemanticMatch::Qualifying(id))
    } else if let Some(id) = unresolved_claim_id {
        Ok(ExistingSemanticMatch::Unresolved(id))
    } else {
        Ok(ExistingSemanticMatch::None)
    }
}

fn record_provenance_review(
    tx: &Transaction<'_>,
    finding_id: i64,
    corroboration: &super::evidence_policy::CorroborationCheck,
) -> io::Result<()> {
    let provenance_review = corroboration
        .provenance_link
        .as_ref()
        .cloned()
        .or_else(|| {
            corroboration.possible_copy_of.map(|related_id| {
                (
                    related_id,
                    "passage substantially overlaps the earlier source; possible copied reporting"
                        .into(),
                )
            })
        })
        .or_else(|| {
            corroboration.shared_source_with.map(|related_id| {
                (
                    related_id,
                    "both findings identify the same verified underlying source".into(),
                )
            })
        });
    if let Some((related_finding_id, provenance_signal)) = provenance_review {
        tx.execute(
            "INSERT OR IGNORE INTO scholar_provenance_reviews
                     (finding_id, related_finding_id, signal, status, created_at)
                     VALUES (?1, ?2, ?3, 'pending', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![finding_id, related_finding_id, provenance_signal],
        )
        .map_err(database_error)?;
    }
    Ok(())
}

fn ingest_source(
    tx: &Transaction<'_>,
    job_id: &str,
    source: &ScholarEvidence,
) -> io::Result<Option<IngestedFinding>> {
    let ScholarEvidence {
        source_title,
        source_url,
        source_excerpt,
        finding,
        profile,
        research_relevance,
    } = source;
    let Some(finding) = finding.as_ref() else {
        return Ok(None);
    };
    let mut reason;
    let model_derived_lead: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM scout_investigations
                 WHERE scholar_job_id = ?1 AND trigger_kind = 'model_probe')
             OR EXISTS(SELECT 1 FROM scholar_research_feedback WHERE scholar_job_id = ?1)",
            [job_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    let today = crate::storage::current_sql_date(tx)?;
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
    if profile
        .verified_underlying_source_id
        .as_deref()
        .is_some_and(|id| !crate::storage::valid_printable_field(id, 200))
    {
        return Err(invalid("invalid verified source identity"));
    }
    let publication_is_recent =
        crate::storage::publication_date_is_recent(tx, profile.published_at.as_deref())?;
    let source_id = profile
        .verified_underlying_source_id
        .as_deref()
        .filter(|_| !source_url.starts_with("https://en.wikipedia.org/wiki/"))
        .map(canonical);
    let corroboration = super::evidence_policy::check_corroboration(
        tx,
        finding,
        source_excerpt,
        source_url,
        job_id,
        source_id.as_deref(),
    )?;
    let time_sensitive = is_time_sensitive(&question, &finding.relation, &finding.object);
    let freshness = scholar_source_freshness(tx, finding, time_sensitive)?;
    let supported = grounded_scholar_finding(source_title, source_excerpt, finding);
    let mut semantic_id = None;
    let mut conflicting_claim_id = None;
    let mut qualifying_claim_id = None;
    let mut unresolved_claim_id = None;
    let mut finding_status = "needs_review";
    let assessment = super::evidence_policy::assess_scholar_finding_with_relevance(
        super::evidence_policy::FindingAssessmentInput {
            question: &question,
            source_url,
            finding,
            direct_support: supported,
            freshness,
            profile,
            publication_is_recent,
            has_independent_corroboration: !corroboration.independent_sources.is_empty()
                && corroboration.possible_copy_of.is_none()
                && corroboration.provenance_link.is_none(),
            research_relevance,
        },
    );
    let mut gate_status = assessment.gate_status;
    reason = Some(assessment.reason.clone());
    let provenance_warning = corroboration
        .provenance_link
        .as_ref()
        .map(|(earlier_id, link_reason)| {
            (
                *earlier_id,
                format!(
                    "{link_reason} with finding #{earlier_id}; review before counting as independent"
                ),
            )
        })
        .or_else(|| {
            corroboration.possible_copy_of.map(|earlier_id| {
                (
                    earlier_id,
                    format!(
                        "passage substantially overlaps finding #{earlier_id}; independent reporting needs review"
                    ),
                )
            })
        });
    if let Some((_, warning)) = provenance_warning {
        // A warning blocks secondary corroboration without claiming the fact is false.
        if assessment.source_kind == "secondary"
            && (gate_status == "eligible" || gate_status == "awaiting_corroboration")
        {
            gate_status = "possible_shared_source";
        }
        reason = Some(format!("{}; {warning}", assessment.reason));
    } else if let Some(earlier_id) = corroboration.shared_source_with
        && gate_status == "awaiting_corroboration"
    {
        reason = Some(format!(
            "shares verified underlying source with finding #{earlier_id}; counts as one source"
        ));
    }
    let personal_web_claim = matches!(canonical(&finding.subject).as_str(), "user" | "the user");
    if personal_web_claim {
        reason = Some("web research cannot establish a personal fact about the user".into());
        gate_status = "pending_review";
    }
    if supported
        && finding.confidence >= PROMOTION_CONFIDENCE
        && assessment.source_kind != "unknown"
        && !personal_web_claim
    {
        match existing_semantic_match(tx, finding, &today)? {
            ExistingSemanticMatch::Conflict(id) => conflicting_claim_id = Some(id),
            ExistingSemanticMatch::Match(id) => semantic_id = Some(id),
            ExistingSemanticMatch::Qualifying(id) => qualifying_claim_id = Some(id),
            ExistingSemanticMatch::Unresolved(id) => unresolved_claim_id = Some(id),
            ExistingSemanticMatch::None => {}
        }
        if let Some(id) = conflicting_claim_id {
            reason = Some(format!("conflicts with overlapping semantic claim #{id}"));
            gate_status = "conflict";
        }
    }
    if model_derived_lead && gate_status == "eligible" {
        gate_status = "pending_review";
        reason = Some("model-derived lead requires independent corroboration or review".into());
    }
    if gate_status == "eligible" && conflicting_claim_id.is_none() {
        if semantic_id.is_none() {
            semantic_id = Some(super::insert_scholar_semantic_claim(
                tx,
                super::NewSemanticClaim {
                    subject: finding.subject.trim(),
                    relation: finding.relation.trim(),
                    object: finding.object.trim(),
                    source: source_url,
                    supersedes_id: None,
                    valid_from: finding.valid_from.as_deref(),
                    valid_until: finding.valid_until.as_deref(),
                    proposition: &finding.proposition,
                },
            )?);
        }
        finding_status = "promoted";
    }
    let (source_archive_id, evidence_byte_start, evidence_byte_end) = source_version_for_finding(
        tx,
        job_id,
        source_title,
        source_url,
        source_excerpt,
        &finding.evidence_text,
        finding.source_updated_at.as_deref(),
    )?;
    tx.execute(
                "INSERT INTO scholar_findings (job_id, source_title, source_url, source_excerpt,
                  subject, relation, object, evidence_text, confidence, status, semantic_claim_id, decision_reason,
                  valid_from, valid_until, source_quality, freshness, source_updated_at,
                  source_archive_id,evidence_byte_start,evidence_byte_end,attribution,polarity,modality,geographic_scope,context_scope)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
                params![job_id, source_title, source_url, source_excerpt, finding.subject, finding.relation,
                    finding.object, finding.evidence_text, finding.confidence, finding_status, semantic_id, reason,
                    finding.valid_from, finding.valid_until, finding.source_quality, freshness, finding.source_updated_at,
                    source_archive_id,evidence_byte_start,evidence_byte_end, finding.proposition.attribution,
                    finding.proposition.polarity.as_str(), finding.proposition.modality.as_str(),
                    finding.proposition.geographic_scope, finding.proposition.context_scope],
            ).map_err(database_error)?;
    let finding_id = tx.last_insert_rowid();
    if let Some(claim_id) = semantic_id {
        super::link_scholar_finding_claim(
            tx,
            finding_id,
            claim_id,
            "supports",
            "finding matches the active semantic claim's subject, relation, object, and validity period",
            if finding.semantic_support.is_some() {
                "reviewed_semantic_entailment_and_policy_match"
            } else {
                "deterministic_exact_claim_match"
            },
        )?;
    }
    if let Some(claim_id) = conflicting_claim_id {
        super::link_scholar_finding_claim(
            tx,
            finding_id,
            claim_id,
            "contradicts",
            "same subject and relation with a different object and overlapping validity period; scope comparison may still be required",
            "deterministic_overlapping_claim_match",
        )?;
    }
    if let Some(claim_id) = qualifying_claim_id {
        super::link_scholar_finding_claim(
            tx,
            finding_id,
            claim_id,
            "qualifies",
            "same subject and relation with overlapping time, but attribution, polarity, or modality differs; retained as a qualification rather than a contradiction",
            "deterministic_scope_comparison",
        )?;
    }
    if let Some(claim_id) = unresolved_claim_id {
        super::link_scholar_finding_claim(
            tx,
            finding_id,
            claim_id,
            "unresolved",
            "geographic or contextual scope differs or is unknown; preserve both claims without treating either as support or contradiction",
            "deterministic_scope_mismatch",
        )?;
    }
    {
        let mut statement = tx
            .prepare(
                "SELECT c.id FROM semantic_claims c
             WHERE c.retracted_by IS NULL AND c.status IN ('active','uncertain','challenged')
               AND (lower(trim(c.subject)) = lower(trim(?1)) OR EXISTS (
                    SELECT 1 FROM entity_equivalences e
                    WHERE e.left_name = lower(trim(c.subject))
                      AND e.right_name = lower(trim(?1))))
               AND lower(trim(c.relation)) != lower(trim(?2))
             ORDER BY c.id DESC LIMIT 20",
            )
            .map_err(database_error)?;
        let unresolved_relations = statement
            .query_map(params![finding.subject, finding.relation], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        drop(statement);
        for claim_id in unresolved_relations {
            super::link_scholar_finding_claim(
                tx,
                finding_id,
                claim_id,
                "unresolved",
                "same established subject with different relation wording; semantic review must decide whether the propositions address the same property",
                "relation_requires_semantic_review",
            )?;
        }
    }
    if let Some(support) = finding.semantic_support.as_ref() {
        if !support.supports(finding, source_excerpt) {
            return Err(invalid(
                "semantic support assessment does not match the stored finding",
            ));
        }
        tx.execute("INSERT INTO scholar_semantic_support (finding_id, assessment_json, assessed_at) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))", params![finding_id, serde_json::to_string(support).map_err(invalid_json)?]).map_err(database_error)?;
    }
    crate::diagnostics::event(
        "scholar.finding_disposition",
        serde_json::json!({
            "finding_id": finding_id,
            "job_id": job_id,
            "status": finding_status,
            "gate_status": gate_status,
            "confidence": finding.confidence,
            "support_method": if finding.semantic_support.is_some() { "ai_semantic_entailment" } else { "lexical" },
            "semantic_support": finding.semantic_support,
            "reason": reason,
            "source_url": source_url,
        }),
    );
    super::evidence_policy::record_assessment(
        tx,
        finding_id,
        &assessment,
        gate_status,
        reason.as_deref().unwrap_or("assessment unavailable"),
    )?;
    record_provenance_review(tx, finding_id, &corroboration)?;
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
        "geographic_scope": finding.proposition.geographic_scope,
        "context_scope": finding.proposition.context_scope,
    }))
    .map_err(invalid_json)?;
    let proposal_id = record_role_proposal(
        tx,
        RoleProposal {
            role: "scholar",
            source_type: "scholar_finding",
            source_id: &finding_id.to_string(),
            suggested_action: "add_claim",
            target_claim_id: semantic_id
                .or(conflicting_claim_id)
                .or(qualifying_claim_id)
                .or(unresolved_claim_id),
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
        tx,
        proposal_id,
        outcome,
        reason
            .as_deref()
            .unwrap_or("accepted by source-support policy"),
        semantic_id
            .or(conflicting_claim_id)
            .or(qualifying_claim_id)
            .or(unresolved_claim_id),
        Some(if conflicting_claim_id.is_some() {
            "challenged"
        } else {
            finding_status
        }),
    )?;
    if let Some(claim_id) = semantic_id {
        link_claim_evidence(
            tx,
            claim_id,
            "web",
            Some(source_url),
            None,
            source_excerpt,
            "supports",
        )?;
        if finding_status == "promoted" && assessment.source_kind == "secondary" {
            for earlier in &corroboration.independent_sources {
                link_claim_evidence(
                    tx,
                    claim_id,
                    "web",
                    Some(&earlier.source_url),
                    None,
                    &earlier.source_excerpt,
                    "supports",
                )?;
                let changed = super::promote_scholar_finding(
                    tx,
                    earlier.finding_id,
                    claim_id,
                    "promoted after verified independent corroboration",
                )?;
                if changed == 1 {
                    super::link_scholar_finding_claim(
                        tx,
                        earlier.finding_id,
                        claim_id,
                        "supports",
                        "later finding passed the independent corroboration policy for the same proposition and validity period",
                        "later_verified_corroboration",
                    )?;
                    tx.execute(
                        "UPDATE scholar_jobs SET status = 'promoted', last_error = NULL
                                 WHERE id = ?1 AND status = 'needs_review'",
                        [&earlier.job_id],
                    )
                    .map_err(database_error)?;
                    super::set_scholar_task_stop_reason(tx, &earlier.job_id, "complete")?;
                    let earlier_proposal: Option<i64> = tx
                        .query_row(
                            "SELECT id FROM role_proposals
                                 WHERE source_type = 'scholar_finding' AND source_id = ?1
                                 ORDER BY id DESC LIMIT 1",
                            [earlier.finding_id.to_string()],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(database_error)?;
                    if let Some(proposal_id) = earlier_proposal {
                        record_policy_decision(
                            tx,
                            proposal_id,
                            "applied",
                            "later verified independent source corroborated this finding",
                            Some(claim_id),
                            Some("promoted"),
                        )?;
                    }
                }
            }
        }
        if finding_status == "promoted" {
            super::activate_reviewed_claim(tx, claim_id)?;
        }
    } else if let Some(claim_id) = conflicting_claim_id {
        link_claim_evidence(
            tx,
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
    } else if let Some(claim_id) = qualifying_claim_id {
        link_claim_evidence(
            tx,
            claim_id,
            "web",
            Some(source_url),
            None,
            source_excerpt,
            "qualifies",
        )?;
    }
    Ok(Some(IngestedFinding {
        id: finding_id,
        status: finding_status,
        reason,
    }))
}

impl LearningStore {
    pub fn finish_scholar_job_with_evidence(
        &mut self,
        job_id: &str,
        sources: &[ScholarEvidence],
        note: Option<&str>,
    ) -> io::Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let mut job_status = "no_result";
        let mut job_reason = note.map(str::to_owned);
        let mut new_finding_ids = Vec::new();
        for source in sources {
            let Some(result) = ingest_source(&tx, job_id, source)? else {
                continue;
            };
            new_finding_ids.push(result.id);
            if job_status != "promoted" || result.status == "promoted" {
                job_status = result.status;
                job_reason = result.reason;
            }
        }
        for finding_id in new_finding_ids {
            super::corroboration::enqueue_corroboration_candidate(&tx, finding_id)?;
        }
        let has_findings: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM scholar_findings WHERE job_id = ?1)",
                [job_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let has_unresolved_findings: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM scholar_findings
                 WHERE job_id = ?1 AND status = 'needs_review')",
                [job_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if has_findings && !has_unresolved_findings {
            job_status = "promoted";
            job_reason =
                Some("all directly supported findings passed automatic evidence review".into());
        }
        let changed = tx
            .execute(
                "UPDATE scholar_jobs SET status = ?2, last_error = ?3, claimed_at = NULL
             WHERE id = ?1 AND status = 'processing'",
                params![job_id, job_status, job_reason],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("scholar job is not processing"));
        }
        let stop_reason = match job_status {
            "promoted" => "complete",
            "needs_review" => "grounded_finding_pending_review",
            _ if sources.is_empty() => "no_source_found",
            _ => "source_found_but_insufficient",
        };
        super::set_scholar_task_stop_reason(&tx, job_id, stop_reason)?;
        tx.commit().map_err(database_error)
    }
}

impl LearningStore {
    /// Upgrade a pending lexical assessment using an independently judged source span.
    pub(crate) fn record_scholar_semantic_support(
        &mut self,
        finding_id: i64,
        support: &crate::semantic::SemanticSupport,
    ) -> io::Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let finding = tx.query_row(
            "SELECT subject, relation, object, evidence_text, confidence, valid_from, valid_until, source_quality, source_updated_at, source_excerpt, source_title, attribution, polarity, modality, geographic_scope, context_scope FROM scholar_findings WHERE id = ?1 AND status = 'needs_review'", [finding_id],
            SemanticSupportFinding::from_row,
        ).map_err(database_error)?;
        let SemanticSupportFinding {
            mut finding,
            passage,
            title,
        } = finding;
        if !support.matches_claim(&finding) {
            return Err(invalid("semantic assessment belongs to a different claim"));
        }
        let supported = if support.verdict() == "entailed" {
            finding.evidence_text = support.evidence().to_owned();
            finding.semantic_support = Some(support.clone());
            if !grounded_scholar_finding(&title, &passage, &finding) {
                return Err(invalid(
                    "semantic assessment lacks valid original source support",
                ));
            }
            true
        } else {
            false
        };
        tx.execute("INSERT INTO scholar_semantic_support (finding_id, assessment_json, assessed_at) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')) ON CONFLICT(finding_id) DO UPDATE SET assessment_json = excluded.assessment_json, assessed_at = excluded.assessed_at", params![finding_id,serde_json::to_string(support).map_err(invalid_json)?]).map_err(database_error)?;
        if supported {
            tx.execute(
                "UPDATE scholar_findings SET evidence_text = ?2 WHERE id = ?1",
                params![finding_id, finding.evidence_text],
            )
            .map_err(database_error)?;
        }
        tx.execute("UPDATE scholar_source_assessments SET direct_support = ?2,
            gate_status = CASE WHEN ?2 = 0 THEN 'unsupported' WHEN gate_status = 'unsupported' THEN 'pending_review' ELSE gate_status END,
            reason = CASE WHEN gate_status = 'conflict' THEN reason || '. ' || ?3 ELSE ?3 END WHERE finding_id = ?1", params![finding_id,supported,format!("AI semantic {}: {}",support.verdict(),support.reason())]).map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        crate::diagnostics::event(
            "scholar.semantic_support_recorded",
            serde_json::json!({"finding_id":finding_id,"direct_support":supported,"assessment":support}),
        );
        Ok(())
    }
}
