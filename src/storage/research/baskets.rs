use super::*;

#[derive(Debug)]
pub struct ResearchSource {
    pub id: i64,
    pub title: String,
    pub url: String,
    pub passage: String,
    pub cursor: i64,
    pub next_cursor: i64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RetainedClaim {
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub evidence_text: String,
    #[serde(default)]
    pub qualifications: String,
    #[serde(default, alias = "confidence")]
    pub extraction_confidence: f64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RetainedQuestion {
    pub question: String,
    pub evidence_text: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ResearchBaskets {
    pub claims: Vec<RetainedClaim>,
    #[serde(default)]
    pub questions: Vec<RetainedQuestion>,
}

#[derive(Debug, Serialize)]
pub struct ClaimPair {
    pub left_id: i64,
    pub right_id: i64,
    pub left: serde_json::Value,
    pub right: serde_json::Value,
}

fn now_ms() -> io::Result<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)
        .and_then(|duration| i64::try_from(duration.as_millis()).map_err(io::Error::other))
}

fn bounded_field(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= maximum
        && !value.chars().any(char::is_control)
        && !value.contains("<|")
}

impl LearningStore {
    pub fn bootstrap_research_sources(&self) -> io::Result<i64> {
        let populated: bool = self
            .connection
            .query_row("SELECT EXISTS(SELECT 1 FROM research_sources)", [], |row| {
                row.get(0)
            })
            .map_err(database_error)?;
        if populated {
            return Ok(0);
        }
        self.import_retained_research_sources()
    }

    pub fn import_retained_research_sources(&self) -> io::Result<i64> {
        let before: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM research_sources", [], |row| {
                row.get(0)
            })
            .map_err(database_error)?;
        let mut statement = self.connection.prepare(
            "SELECT job_id,source_title,source_url,source_excerpt FROM scholar_findings
             UNION SELECT job_id,title,COALESCE(fetched_url,url),fetched_excerpt FROM scholar_search_candidates
                WHERE fetched_excerpt IS NOT NULL AND fetched_excerpt<>'' LIMIT 500"
        ).map_err(database_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        drop(statement);
        for (job, title, url, body) in rows {
            self.retain_research_source(&job, &title, &url, &body)?;
        }
        let after: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM research_sources", [], |row| {
                row.get(0)
            })
            .map_err(database_error)?;
        Ok(after - before)
    }

    pub fn retain_fetched_candidate_source(
        &self,
        candidate: i64,
        title: &str,
        url: &str,
        body: &str,
    ) -> io::Result<()> {
        let job: String = self
            .connection
            .query_row(
                "SELECT job_id FROM scholar_search_candidates WHERE id=?1",
                [candidate],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        self.retain_research_source(&job, title, url, body)
    }

    pub fn retain_research_source(
        &self,
        job: &str,
        title: &str,
        url: &str,
        body: &str,
    ) -> io::Result<()> {
        let parsed = url::Url::parse(url).map_err(|_| invalid("invalid research source URL"))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(invalid("invalid research source URL"));
        }
        if body.trim().is_empty() {
            return Ok(());
        }
        let body = body.chars().take(32_000).collect::<String>();
        let hash = format!("{:x}", Sha256::digest(body.as_bytes()));
        self.connection.execute(
            "INSERT OR IGNORE INTO research_sources(job_id,title,url,content_hash,body,created_at)
             VALUES (?1,?2,?3,?4,?5,strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
            params![job,title,url,hash,body],
        ).map_err(database_error)?;
        Ok(())
    }

    pub fn claim_research_source(&mut self) -> io::Result<Option<ResearchSource>> {
        let now = now_ms()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let row: Option<(i64,String,String,String,i64)> = tx.query_row(
            "SELECT id,title,url,body,harvest_cursor FROM research_sources
             WHERE harvest_cursor < length(body) AND claimed_until_ms <= ?1 AND next_attempt_at_ms <= ?1
             ORDER BY id LIMIT 1",[now],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
        ).optional().map_err(database_error)?;
        let Some((id, title, url, body, cursor)) = row else {
            return Ok(None);
        };
        let offset =
            usize::try_from(cursor).map_err(|_| invalid("invalid research source cursor"))?;
        let mut passage = body.chars().skip(offset).take(2200).collect::<String>();
        if body.chars().count() > offset + passage.chars().count() {
            if let Some(end) = passage.rfind('\n').filter(|end| *end > 1000) {
                passage.truncate(end + 1);
            }
        }
        let next_cursor =
            cursor + i64::try_from(passage.chars().count()).map_err(io::Error::other)?;
        tx.execute(
            "UPDATE research_sources SET claimed_until_ms=?2 WHERE id=?1",
            params![id, now + 600_000],
        )
        .map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        Ok(Some(ResearchSource {
            id,
            title,
            url,
            passage,
            cursor,
            next_cursor,
        }))
    }

    pub fn finish_research_baskets(
        &mut self,
        source: &ResearchSource,
        baskets: &ResearchBaskets,
    ) -> io::Result<(usize, usize)> {
        if baskets.claims.len() > 3 || baskets.questions.len() > 2 {
            return Err(invalid(
                "research baskets exceed 3 claims or 2 questions per passage",
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let (body, job, cursor): (String, String, i64) = tx
            .query_row(
                "SELECT body,job_id,harvest_cursor FROM research_sources WHERE id=?1",
                [source.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(database_error)?;
        let offset =
            usize::try_from(cursor).map_err(|_| invalid("invalid research source cursor"))?;
        if cursor != source.cursor
            || source.next_cursor
                != source.cursor
                    + i64::try_from(source.passage.chars().count()).map_err(io::Error::other)?
            || body
                .chars()
                .skip(offset)
                .take(source.passage.chars().count())
                .collect::<String>()
                != source.passage
        {
            return Err(invalid("research source cursor or passage changed"));
        }
        let mut claims = 0;
        for claim in &baskets.claims {
            if ![&claim.subject, &claim.relation, &claim.object]
                .iter()
                .all(|field| bounded_field(field, 500))
                || claim.qualifications.chars().count() > 500
                || claim.evidence_text.trim().is_empty()
                || !source.passage.contains(&claim.evidence_text)
                || !claim.extraction_confidence.is_finite()
                || !(0.0..=1.0).contains(&claim.extraction_confidence)
            {
                return Err(invalid(
                    "retained claim must be bounded and quoted from its source",
                ));
            }
            claims += tx.execute("INSERT OR IGNORE INTO research_claims(source_id,subject,relation,object,subject_key,relation_key,evidence_text,qualifications,extraction_confidence,created_at)
                VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
                params![source.id,claim.subject.trim(),claim.relation.trim(),claim.object.trim(),canonical(&claim.subject),canonical(&claim.relation),claim.evidence_text,claim.qualifications,claim.extraction_confidence]).map_err(database_error)?;
        }
        let depth: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(depth),0)+1 FROM research_questions WHERE scholar_job_id=?1",
                [&job],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let mut questions = 0;
        for question in &baskets.questions {
            if !bounded_field(&question.question, 300)
                || question.evidence_text.trim().is_empty()
                || !source.passage.contains(&question.evidence_text)
            {
                return Err(invalid(
                    "research question requires a bounded question and exact source evidence",
                ));
            }
            if crate::research::private_fact_request(&question.question)
                || question.question.to_ascii_lowercase().contains("the user")
                || depth > 3
            {
                continue;
            }
            let key = canonical(&question.question);
            questions += tx.execute("INSERT OR IGNORE INTO research_questions(source_id,question,question_key,evidence_text,depth,created_at)
                VALUES (?1,?2,?3,?4,?5,strftime('%Y-%m-%dT%H:%M:%fZ','now'))",params![source.id,question.question.trim(),key,question.evidence_text,depth]).map_err(database_error)?;
            tx.execute("INSERT OR IGNORE INTO research_question_sources(question_id,source_id,evidence_text)
                SELECT id,?2,?3 FROM research_questions WHERE question_key=?1",params![key,source.id,question.evidence_text]).map_err(database_error)?;
        }
        tx.execute("UPDATE research_sources SET harvest_cursor=?2,claimed_until_ms=0,last_error=NULL,next_attempt_at_ms=0 WHERE id=?1",params![source.id,source.next_cursor]).map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        Ok((claims, questions))
    }

    pub fn defer_research_source(&self, id: i64, reason: &str) -> io::Result<()> {
        self.connection.execute("UPDATE research_sources SET claimed_until_ms=0,next_attempt_at_ms=?2,last_error=?3 WHERE id=?1",
            params![id,now_ms()?+300_000,reason.chars().take(500).collect::<String>()]).map_err(database_error)?;
        Ok(())
    }

    pub fn queue_retained_question(&mut self) -> io::Result<bool> {
        self.connection.execute("UPDATE research_questions SET status=CASE
                WHEN (SELECT status FROM scholar_jobs WHERE id=scholar_job_id)='no_result' THEN 'unanswered' ELSE 'researched' END
            WHERE status='queued' AND scholar_job_id IN (SELECT id FROM scholar_jobs WHERE status IN ('promoted','needs_review','no_result'))",[]).map_err(database_error)?;
        if self.has_pending_user_research()? {
            return Ok(false);
        }
        let queued: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM research_questions WHERE status='queued'",
                [],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if queued >= 20 {
            return Ok(false);
        }
        let question: Option<(i64,i64,String)> = self.connection.query_row("SELECT id,source_id,question FROM research_questions WHERE status='pending' ORDER BY id LIMIT 1",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(database_error)?;
        let Some((id, source, question)) = question else {
            return Ok(false);
        };
        let key = format!("source-question:{id}");
        if !self.enqueue_scout_research_with_reason(
            "expansion",
            &key,
            &format!("source:{source}"),
            &question,
            "Retained source passage raises a grounded follow-up question",
        )? {
            return Ok(false);
        }
        self.connection.execute("UPDATE research_questions SET status='queued',scholar_job_id=(SELECT scholar_job_id FROM scout_investigations WHERE trigger_kind='expansion' AND topic_key=?2 ORDER BY id DESC LIMIT 1) WHERE id=?1",params![id,key]).map_err(database_error)?;
        crate::diagnostics::event(
            "research.question_queued",
            serde_json::json!({"question_id":id,"source_id":source}),
        );
        Ok(true)
    }

    pub fn next_research_claim_pair(&self) -> io::Result<Option<ClaimPair>> {
        self.connection.query_row(
            "SELECT a.id,b.id,a.subject,a.relation,a.object,a.evidence_text,a.qualifications,sa.url,
                b.subject,b.relation,b.object,b.evidence_text,b.qualifications,sb.url
             FROM research_claims a JOIN research_claims b ON a.id<b.id AND a.subject_key=b.subject_key
             JOIN research_sources sa ON sa.id=a.source_id JOIN research_sources sb ON sb.id=b.source_id
             LEFT JOIN research_evidence_links l ON l.left_claim_id=a.id AND l.right_claim_id=b.id
             WHERE sa.url<>sb.url AND (l.left_claim_id IS NULL OR (l.relation='insufficient' AND l.next_attempt_at_ms>0 AND l.next_attempt_at_ms<=?1))
                 AND (l.left_claim_id IS NOT NULL OR
                    ((SELECT COUNT(*) FROM research_evidence_links WHERE left_claim_id=a.id OR right_claim_id=a.id)<8
                    AND (SELECT COUNT(*) FROM research_evidence_links WHERE left_claim_id=b.id OR right_claim_id=b.id)<8))
             ORDER BY a.id,b.id LIMIT 1",[now_ms()?],|row|{
                let claim = |offset| -> rusqlite::Result<serde_json::Value> {Ok(serde_json::json!({
                    "subject":row.get::<_,String>(offset)?,"relation":row.get::<_,String>(offset+1)?,"object":row.get::<_,String>(offset+2)?,
                    "evidence_text":row.get::<_,String>(offset+3)?,"qualifications":row.get::<_,String>(offset+4)?,"source_url":row.get::<_,String>(offset+5)?,
                }))};
                Ok(ClaimPair {left_id:row.get(0)?,right_id:row.get(1)?,left:claim(2)?,right:claim(8)?})
            },
        ).optional().map_err(database_error)
    }

    pub fn record_research_evidence_link(
        &self,
        pair: &ClaimPair,
        relation: &str,
        rationale: &str,
        reviewer: &str,
        retry: bool,
    ) -> io::Result<()> {
        if !matches!(
            relation,
            "supports" | "contradicts" | "qualifies" | "unrelated" | "insufficient"
        ) || !bounded_field(rationale, 500)
        {
            return Err(invalid("invalid research evidence relationship"));
        }
        self.connection.execute("INSERT INTO research_evidence_links(left_claim_id,right_claim_id,relation,rationale,reviewer_model_id,next_attempt_at_ms,created_at)
            VALUES (?1,?2,?3,?4,?5,?6,strftime('%Y-%m-%dT%H:%M:%fZ','now'))
            ON CONFLICT(left_claim_id,right_claim_id) DO UPDATE SET relation=excluded.relation,rationale=excluded.rationale,reviewer_model_id=excluded.reviewer_model_id,next_attempt_at_ms=excluded.next_attempt_at_ms",
            params![pair.left_id,pair.right_id,relation,rationale,reviewer,if retry {now_ms()?+300_000} else {0}]).map_err(database_error)?;
        Ok(())
    }

    pub fn research_basket_summary(&self) -> io::Result<serde_json::Value> {
        let count = |table: &str| {
            self.connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(database_error)
        };
        Ok(
            serde_json::json!({"sources":count("research_sources")?,"unverified_claims":count("research_claims")?,"questions":count("research_questions")?,"evidence_links":count("research_evidence_links")?}),
        )
    }

    pub fn retained_claim_evidence(&self, id: i64) -> io::Result<serde_json::Value> {
        let (subject,relation,object,quote,qualifications,confidence,url): (String,String,String,String,String,f64,String) = self.connection.query_row(
            "SELECT c.subject,c.relation,c.object,c.evidence_text,c.qualifications,c.extraction_confidence,s.url
             FROM research_claims c JOIN research_sources s ON s.id=c.source_id WHERE c.id=?1",[id],
            |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?)),
        ).map_err(database_error)?;
        let mut statement = self.connection.prepare("SELECT l.relation,l.rationale,s.url,l.reviewer_model_id FROM research_evidence_links l
            JOIN research_claims other ON other.id=CASE WHEN l.left_claim_id=?1 THEN l.right_claim_id ELSE l.left_claim_id END
            JOIN research_sources s ON s.id=other.source_id WHERE (l.left_claim_id=?1 OR l.right_claim_id=?1) AND l.next_attempt_at_ms=0").map_err(database_error)?;
        let links = statement
            .query_map([id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        let group = |url: &str| {
            url::Url::parse(url)
                .ok()
                .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
                .unwrap_or_else(|| url.to_owned())
        };
        let mut groups = std::collections::HashMap::<String, u8>::new();
        groups.insert(group(&url), 1);
        let mut relationships = Vec::new();
        for (kind, rationale, url, reviewer) in links {
            let bit = match kind.as_str() {
                "supports" => 1,
                "contradicts" => 2,
                "qualifies" => 4,
                _ => 0,
            };
            if bit > 0 {
                *groups.entry(group(&url)).or_default() |= bit;
            }
            relationships.push(serde_json::json!({"relation":kind,"rationale":rationale,"source_url":url,"reviewer_model_id":reviewer}));
        }
        let supporting = groups
            .values()
            .filter(|value| **value & 1 != 0 && **value & 2 == 0)
            .count();
        let contradicting = groups
            .values()
            .filter(|value| **value & 2 != 0 && **value & 1 == 0)
            .count();
        let disputed = groups.values().filter(|value| **value & 3 == 3).count();
        let qualified = groups.values().filter(|value| **value & 4 != 0).count();
        let support_score =
            supporting as f64 / (supporting + contradicting + disputed + qualified + 2) as f64;
        Ok(
            serde_json::json!({"id":id,"subject":subject,"relation":relation,"object":object,"evidence_text":quote,
            "qualifications":qualifications,"extraction_confidence":confidence,"source_url":url,
            "status":"unverified","supporting_host_groups":supporting,"contradicting_host_groups":contradicting,
            "disputed_host_groups":disputed,"qualifying_host_groups":qualified,"provisional_support_score":support_score,
            "independence_verified":false,"relationships":relationships}),
        )
    }

    pub fn research_basket_items(&self, kind: &str) -> io::Result<Vec<serde_json::Value>> {
        let sql = match kind {
            "claims" => {
                "SELECT json_object('id',c.id,'subject',c.subject,'relation',c.relation,'object',c.object,'extraction_confidence',c.extraction_confidence,'source_id',c.source_id,'status','unverified') FROM research_claims c ORDER BY c.id DESC LIMIT 50"
            }
            "questions" => {
                "SELECT json_object('id',id,'question',question,'source_id',source_id,'depth',depth,'status',status,'scholar_job_id',scholar_job_id) FROM research_questions ORDER BY id DESC LIMIT 50"
            }
            "sources" => {
                "SELECT json_object('id',id,'title',title,'url',url,'characters',length(body),'harvest_cursor',harvest_cursor,'next_attempt_at_ms',next_attempt_at_ms,'last_error',last_error) FROM research_sources ORDER BY id DESC LIMIT 50"
            }
            _ => return Err(invalid("basket must be claims, questions, or sources")),
        };
        let mut statement = self.connection.prepare(sql).map_err(database_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        rows.into_iter()
            .map(|raw| serde_json::from_str(&raw).map_err(invalid_json))
            .collect()
    }

    pub fn retained_source_detail(&self, id: i64) -> io::Result<serde_json::Value> {
        let raw: String = self.connection.query_row("SELECT json_object('id',id,'job_id',job_id,'title',title,'url',url,'body',body,'harvest_cursor',harvest_cursor,'last_error',last_error) FROM research_sources WHERE id=?1",[id],|row|row.get(0)).map_err(database_error)?;
        serde_json::from_str(&raw).map_err(invalid_json)
    }

    pub fn retry_retained_source(&self, id: i64) -> io::Result<()> {
        let changed = self.connection.execute("UPDATE research_sources SET harvest_cursor=0,next_attempt_at_ms=0,last_error=NULL WHERE id=?1 AND claimed_until_ms<=?2",params![id,now_ms()?]).map_err(database_error)?;
        if changed != 1 {
            return Err(invalid(
                "source does not exist or is currently being harvested",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_excerpts_import_without_refetching_or_changing_review_status() {
        let mut store = LearningStore::open(Path::new(":memory:")).unwrap();
        let plan = crate::research::ResearchPlan {
            question: "Where is the Amazon rainforest?".into(),
            queries: vec!["Amazon rainforest".into()],
        };
        store.enqueue_research_plan("import", 1, &plan).unwrap();
        let job = store
            .claim_conversation_research("import", 1)
            .unwrap()
            .unwrap();
        store.connection.execute("INSERT INTO scholar_findings(job_id,source_title,source_url,source_excerpt,subject,relation,object,evidence_text,confidence,status)
            VALUES (?1,'Amazon','https://example.org/page','The Amazon rainforest is in South America.','Amazon rainforest','is in','South America','The Amazon rainforest is in South America.',0.98,'rejected')",[&job.id]).unwrap();
        assert_eq!(store.bootstrap_research_sources().unwrap(), 1);
        assert_eq!(store.bootstrap_research_sources().unwrap(), 0);
        assert_eq!(store.import_retained_research_sources().unwrap(), 0);
        assert_eq!(
            store.scholar_findings(&job.id).unwrap()[0].status,
            "rejected"
        );
        assert!(store.active_semantic_claims().unwrap().is_empty());
        let requests: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM host_request_log", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(requests, 0);
    }

    fn source(store: &mut LearningStore, url: &str, passage: &str) -> ResearchSource {
        let plan = crate::research::ResearchPlan {
            question: "Where is the Amazon rainforest?".into(),
            queries: vec!["Amazon rainforest".into()],
        };
        store.enqueue_research_plan("baskets", 1, &plan).unwrap();
        let job: String = store
            .connection
            .query_row("SELECT id FROM scholar_jobs LIMIT 1", [], |row| row.get(0))
            .unwrap();
        store
            .retain_research_source(&job, "Amazon rainforest", url, passage)
            .unwrap();
        store.claim_research_source().unwrap().unwrap()
    }

    #[test]
    fn fact_shaped_false_assertions_are_retained_without_promotion() {
        let mut store = LearningStore::open(Path::new(":memory:")).unwrap();
        let source = source(
            &mut store,
            "https://example.org/article",
            "The Amazon rainforest is made of chocolate.",
        );
        let baskets = ResearchBaskets {
            claims: vec![RetainedClaim {
                subject: "Amazon rainforest".into(),
                relation: "is made of".into(),
                object: "chocolate".into(),
                evidence_text: source.passage.clone(),
                qualifications: String::new(),
                extraction_confidence: 0.9,
            }],
            questions: vec![RetainedQuestion {
                question: "What materials form the Amazon rainforest?".into(),
                evidence_text: source.passage.clone(),
            }],
        };
        assert_eq!(
            store.finish_research_baskets(&source, &baskets).unwrap(),
            (1, 1)
        );
        assert!(store.active_semantic_claims().unwrap().is_empty());
        assert!(
            store
                .scholar_findings(
                    &store
                        .connection
                        .query_row::<String, _, _>(
                            "SELECT id FROM scholar_jobs LIMIT 1",
                            [],
                            |row| row.get(0)
                        )
                        .unwrap()
                )
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn unsupported_quotes_rollback_both_baskets() {
        let mut store = LearningStore::open(Path::new(":memory:")).unwrap();
        let source = source(
            &mut store,
            "https://example.org/article",
            "The Amazon rainforest is in South America.",
        );
        let baskets = ResearchBaskets {
            claims: vec![],
            questions: vec![RetainedQuestion {
                question: "Why is it diverse?".into(),
                evidence_text: "invented evidence".into(),
            }],
        };
        assert!(store.finish_research_baskets(&source, &baskets).is_err());
        assert_eq!(store.research_basket_summary().unwrap()["questions"], 0);
    }

    #[test]
    fn retained_content_is_idempotent_and_processed_in_bounded_chunks() {
        let mut store = LearningStore::open(Path::new(":memory:")).unwrap();
        let passage = "A sourced assertion.\n".repeat(200);
        let first = source(&mut store, "https://example.org/article", &passage);
        assert!(first.passage.chars().count() <= 2200);
        assert!(store.claim_research_source().unwrap().is_none());
        store
            .finish_research_baskets(&first, &ResearchBaskets::default())
            .unwrap();
        let second = store.claim_research_source().unwrap().unwrap();
        assert_eq!(second.cursor, first.next_cursor);
        assert!(second.next_cursor > first.next_cursor);
    }

    #[test]
    fn retained_questions_deduplicate_and_respect_user_priority() {
        let mut store = LearningStore::open(Path::new(":memory:")).unwrap();
        let text = "The Amazon rainforest is in South America.";
        let first = source(&mut store, "https://one.example/article", text);
        let baskets = ResearchBaskets {
            claims: vec![],
            questions: vec![RetainedQuestion {
                question: "Why is the Amazon rainforest biodiverse?".into(),
                evidence_text: text.into(),
            }],
        };
        store.finish_research_baskets(&first, &baskets).unwrap();
        let second = source(&mut store, "https://two.example/article", text);
        assert_eq!(
            store.finish_research_baskets(&second, &baskets).unwrap(),
            (0, 0)
        );
        assert!(!store.queue_retained_question().unwrap());
        assert_eq!(store.research_basket_summary().unwrap()["questions"], 1);
        let provenance: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM research_question_sources",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(provenance, 2);
        let question_id: i64 = store
            .connection
            .query_row("SELECT id FROM research_questions", [], |row| row.get(0))
            .unwrap();
        let key = format!("source-question:{question_id}");
        store
            .connection
            .execute("UPDATE scholar_jobs SET status='no_result'", [])
            .unwrap();
        store.connection.execute("INSERT INTO scout_investigations(trigger_kind,topic_key,trigger_ref,question,scholar_job_id,selection_reason,created_at)
            SELECT 'expansion',?1,'source:fixture','fixture',id,'fixture',strftime('%Y-%m-%dT%H:%M:%fZ','now','-8 days') FROM scholar_jobs LIMIT 1",[&key]).unwrap();
        assert!(store.queue_retained_question().unwrap());
        assert_eq!(
            store.research_basket_items("questions").unwrap()[0]["status"],
            "queued"
        );
    }

    #[test]
    fn support_counts_hosts_once_and_contradictions_lower_the_score() {
        let mut store = LearningStore::open(Path::new(":memory:")).unwrap();
        let mut ids = Vec::new();
        for (url, object) in [
            ("https://one.example/a", "South America"),
            ("https://one.example/b", "South America"),
            ("https://two.example/a", "South America"),
            ("https://three.example/a", "Africa"),
        ] {
            let text = format!("The Amazon rainforest is in {object}.");
            let source = source(&mut store, url, &text);
            let baskets = ResearchBaskets {
                claims: vec![RetainedClaim {
                    subject: "Amazon rainforest".into(),
                    relation: "is in".into(),
                    object: object.into(),
                    evidence_text: text,
                    qualifications: String::new(),
                    extraction_confidence: 0.98,
                }],
                questions: vec![],
            };
            store.finish_research_baskets(&source, &baskets).unwrap();
            let id: i64 = store
                .connection
                .query_row(
                    "SELECT id FROM research_claims WHERE source_id=?1",
                    [source.id],
                    |row| row.get(0),
                )
                .unwrap();
            ids.push(id);
        }
        for other in [ids[1], ids[2]] {
            let pair = ClaimPair {
                left_id: ids[0],
                right_id: other,
                left: serde_json::Value::Null,
                right: serde_json::Value::Null,
            };
            store
                .record_research_evidence_link(
                    &pair,
                    "supports",
                    "The quotations express the same location.",
                    "mock-reviewer",
                    false,
                )
                .unwrap();
        }
        let before = store.retained_claim_evidence(ids[0]).unwrap();
        assert_eq!(before["supporting_host_groups"], 2);
        let pair = ClaimPair {
            left_id: ids[0],
            right_id: ids[3],
            left: serde_json::Value::Null,
            right: serde_json::Value::Null,
        };
        store
            .record_research_evidence_link(
                &pair,
                "contradicts",
                "The same rainforest is placed on a different continent.",
                "mock-reviewer",
                false,
            )
            .unwrap();
        let after = store.retained_claim_evidence(ids[0]).unwrap();
        assert_eq!(after["contradicting_host_groups"], 1);
        assert!(
            after["provisional_support_score"].as_f64().unwrap()
                < before["provisional_support_score"].as_f64().unwrap()
        );
        assert_eq!(after["independence_verified"], false);
        assert!(store.active_semantic_claims().unwrap().is_empty());
    }
}
