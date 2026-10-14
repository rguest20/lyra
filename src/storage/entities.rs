use super::*;
use std::collections::HashSet;

/// Direct reviewed links only: no substring matching or transitive closure.
/// Scoped links are available only when callers provide the exact scope.
#[derive(Default)]
pub(crate) struct EntityAliases {
    global: HashSet<(String, String)>,
    #[allow(dead_code)]
    scoped: HashSet<(String, String, String)>,
}
impl EntityAliases {
    pub(crate) fn same(&self, left: &str, right: &str) -> bool {
        let pair = (canonical(left), canonical(right));
        pair.0 == pair.1 || self.global.contains(&pair)
    }
    #[allow(dead_code)]
    pub(crate) fn same_in_scope(&self, left: &str, right: &str, scope: &str) -> bool {
        let left = canonical(left);
        let right = canonical(right);
        let scope = canonical(scope);
        left == right
            || self
                .scoped
                .contains(&(left.clone(), right.clone(), scope.clone()))
            || self.scoped.contains(&(right, left, scope))
    }
    pub(crate) fn mentioned(&self, name: &str, passage: &str) -> bool {
        crate::entities::mentions_name(passage, name)
            || self.global.iter().any(|(left, right)| {
                left == &canonical(name) && crate::entities::mentions_name(passage, right)
            })
    }
    pub(crate) fn load(connection: &Connection) -> io::Result<Self> {
        let mut statement = connection
            .prepare("SELECT left_name,right_name FROM entity_equivalences")
            .map_err(database_error)?;
        let global = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(database_error)?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(database_error)?;
        let mut statement = connection
            .prepare(
                "SELECT l.left_name,l.right_name,l.identity_scope
                 FROM entity_alias_links l
                 WHERE l.identity_scope IS NOT NULL AND (l.source_finding_id IS NULL OR EXISTS (
                    SELECT 1 FROM scholar_findings f WHERE f.id=l.source_finding_id
                      AND (f.status='needs_review' OR (f.status='promoted' AND EXISTS (
                        SELECT 1 FROM semantic_claims c WHERE c.id=f.semantic_claim_id
                          AND c.retracted_by IS NULL AND c.status IN ('active','uncertain'))))))",
            )
            .map_err(database_error)?;
        let scoped = statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get::<_, String>(2)?))
            })
            .map_err(database_error)?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(database_error)?
            .into_iter()
            .map(|(left, right, scope)| (left, right, canonical(&scope)))
            .collect();
        Ok(Self { global, scoped })
    }
}
impl LearningStore {
    pub(crate) fn entity_aliases(&self) -> io::Result<EntityAliases> {
        EntityAliases::load(&self.connection)
    }
    pub(crate) fn record_entity_alias(
        &self,
        source_finding_id: Option<i64>,
        source_url: &str,
        passage: &str,
        alias: &crate::entities::ReviewedAlias,
    ) -> io::Result<()> {
        let finding = &alias.finding;
        let support = finding
            .semantic_support
            .as_ref()
            .filter(|s| s.supports(finding, passage))
            .ok_or_else(|| invalid("entity alias needs claim-bound semantic support"))?;
        if finding.relation != "is the same entity as"
            || alias.identity_scope != finding.proposition.context_scope
            || finding.valid_from.is_some()
            || finding.valid_until.is_some()
            || source_url.trim().is_empty()
        {
            return Err(invalid("invalid entity identity link"));
        }
        if alias.identity_scope.as_deref().is_some_and(|scope| {
            scope.trim().is_empty()
                || scope.chars().count() > 240
                || scope.chars().any(char::is_control)
                || !passage.contains(scope)
                || !finding.evidence_text.contains(scope)
        }) {
            return Err(invalid("identity scope must be an exact source quotation"));
        }
        if let Some(id) = source_finding_id {
            let matches: bool = self.connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM scholar_findings WHERE id=?1 AND source_url=?2 AND instr(source_excerpt,?3)>0 AND status IN ('needs_review','promoted'))",
                params![id,source_url,finding.evidence_text],|row|row.get(0)).map_err(database_error)?;
            if !matches {
                return Err(invalid("alias must refer to its original finding source"));
            }
        }
        self.connection.execute("INSERT OR IGNORE INTO entity_alias_links (left_name,right_name,source_url,evidence_text,assessment_json,source_finding_id,identity_scope) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![canonical(&finding.subject),canonical(&finding.object),source_url,finding.evidence_text,serde_json::to_string(support).map_err(invalid_json)?,source_finding_id,alias.identity_scope]).map_err(database_error)?;
        crate::diagnostics::event(
            "entity.alias_recorded",
            serde_json::json!({"name":finding.subject,"alias":finding.object,"source_finding_id":source_finding_id,"source":crate::diagnostics::safe_public_url(source_url)}),
        );
        Ok(())
    }
}
