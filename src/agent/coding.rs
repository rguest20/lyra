//! Conversation entry to Scholar's coding loop, preserving source-check requirements.
use super::*;

impl Agent {
    pub(super) fn solve_coding_task(
        &self,
        request: &task::ResolvedTurnRequest<'_>,
        services: &mut ReplyServices<'_>,
    ) -> io::Result<(String, bool)> {
        let mut sources = crate::coding::SourceContext {
            required: request.needs_external_facts || request.requires_source_check,
            ..Default::default()
        };
        let mut researched = false;
        if sources.required {
            let result = (|| {
                let plan = self
                    .plan_research(
                        request,
                        "No code has been implemented yet.",
                        &[],
                        services.research_model,
                    )?
                    .ok_or_else(|| invalid("required code prerequisites have no research plan"))?;
                services.store.enqueue_research_plan(
                    &self.conversation_id,
                    self.turn_count() + 1,
                    &plan,
                )?;
                researched = true;
                crate::scholar::process_conversation(
                    services.database_path,
                    &self.conversation_id,
                    self.turn_count() + 1,
                    services.scholar_model,
                )?;
                let evidence = semantic_research_evidence(
                    services.store,
                    &request.resolved_request,
                    services.research_model,
                )?;
                for finding in evidence.into_iter().take(6) {
                    sources.evidence.push(json!({
                        "finding_id":finding.id,"source":finding.source_url,
                        "evidence_text":finding.evidence_text,"published_at":finding.published_at,
                        "status":finding.finding_status,"gate_status":finding.gate_status,
                        "attribution":finding.proposition.attribution,
                        "geographic_scope":finding.proposition.geographic_scope,
                        "context_scope":finding.proposition.context_scope,
                        "polarity":finding.proposition.polarity.as_str(),
                        "modality":finding.proposition.modality.as_str(),
                        "source_archive_id":finding.source_archive_id,
                        "evidence_byte_start":finding.evidence_byte_start,"evidence_byte_end":finding.evidence_byte_end,
                        "caveats":finding.review_caveats,
                        "conflicts":finding.conflicting_claims,
                    }));
                }
                if sources.evidence.is_empty() {
                    return Err(invalid(
                        "required external prerequisites remain unsupported",
                    ));
                }
                Ok(())
            })();
            if let Err(error) = result {
                sources.limitation = Some(error.to_string());
            }
        }
        let root = crate::scholar::coding::runs_root(services.database_path);
        let (report, directory) = crate::scholar::coding::solve(
            request.original_message,
            &self.history,
            &root,
            services.scholar_model,
            services.research_model,
            sources,
        )?;
        Ok((report.summary(&directory), researched))
    }
}
