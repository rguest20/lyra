//! Scholar's bounded hypothesis → artifact → execution → revision cycle.
use crate::agent::Message;
use crate::coding::*;
use crate::curmudgeon::coding as reviewer;
use crate::inference::LocalModel;
use serde_json::json;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

const PLAN_PROMPT: &str = r#"You are Scholar solving the user's problem by writing and executing code. First hypothesize how to solve it. Preserve the original request, its constraints, negative requirements and conversation attribution. Decompose the full deliverable into 1–8 testable requirements with unique IDs. Do not quietly narrow scope to fit the environment. Explain assumptions and any infeasible parts. Set requires_external_evidence if completion depends on external facts, current documentation or explicit source verification; execution cannot establish these prerequisites. The first runtime is offline Python 3.12 with its standard library. The public artifact must include solution.py, with additional relative files allowed. You will get bounded execution feedback and may revise; a hypothesis is not a success declaration.
Return {"hypothesis":"proposed explanation of how code solves the problem","requirements":[{"id":"r1","description":"requested behavior"}],"assumptions":["assumption"],"approach":"implementation and evaluation approach","requires_external_evidence":true|false}."#;

const BUILD_PROMPT: &str = r#"You are Scholar implementing the original request using the frozen plan and acceptance interface. Produce a complete replacement artifact, not a patch or shell command. Include solution.py. Use Python 3.12 standard library only. Additional files use unique relative paths without traversal; test_ paths are reserved. At most 12 files and 48 KiB total source. This stage writes ONLY the implementation; a separate stage writes the tests. Keep the implementation focused on the requested behavior. Nothing may rely on network, host files, extra packages or persistent state between executions. Respect the original task's constraints, not merely the known examples. Do not bypass tests, forge output or mask failures. Explain the current hypothesis and what the retained feedback changes.
Return {"hypothesis":"current proposed solution","change_reason":"why this implementation or revision follows from evidence","files":[{"path":"solution.py","content":"complete Python implementation, without test suites"}]}."#;

const TESTS_PROMPT: &str = r#"You are Scholar writing checks for the supplied implementation and original request. Return 1–8 focused test cases as structured data, not a test module. The runner supplies a unittest.TestCase, the solution module, and its public names. Each body is Python statements inside one test method, with self.assert... methods available. Standard-library imports may appear inside a body. Do not repeat the implementation, imports from solution, class definitions or a main block. Derive expected values from the original requirements, not the implementation's behavior. Cover distinct requested behavior and relevant boundaries without inventing input restrictions. Each name is a unique ASCII identifier; each body is at most 1536 bytes. Each suite runs in a fresh offline container.
Return {"reason":"what these checks establish and their limits","cases":[{"name":"behavior_identifier","body":"self.assertEqual(solution.requested_function(...), expected_value)"}]}."#;

const ASSESS_PROMPT: &str = r#"You are Scholar assessing your implemented hypothesis after actual sandbox execution. Compare the original request, code, tests and both execution results. Passing tests do not alone prove the full task solved. Distinguish implementation errors, test defects, insufficient coverage and infrastructure failures. State what is demonstrated and what remains hypothetical. Classify complete, partial (useful requested parts), limited (useful only within narrower conditions), not_solved, or unresolved. Explain why it worked or failed and the next concrete change. Do not treat your own agreement as independent confirmation.
Return {"outcome":"complete|partial|limited|not_solved|unresolved","reason":"evidence-based self-assessment","limitations":["explicit unresolved limits"],"next_change":"next revision or reason to stop"}."#;

const TEST_AUDIT_PROMPT: &str = r#"You are Scholar checking Curmudgeon's proposed acceptance suite BEFORE writing the implementation. Audit functional test validity, not coding style. Use the original request directly; the plan and assumptions may be mistaken. Independently derive the expected results and check that rejection conditions follow from the user's actual constraints. Verify that the suite imports the implementation interface it calls, can be collected by unittest, and contains assertions that exercise the claimed requirements. Do not approve a suite merely because it looks like a familiar example. Do not invent extra restrictions or change the intended task to make the suite convenient. No implementation is available, so do not adjust tests to favor a particular candidate. Accept means the suite is usable for the requested behavior; revise means identify concrete defects and corrections; unresolved means you cannot establish validity. This is another same-model assessment, not independent confirmation.
Return {"verdict":"accept|revise|unresolved","reason":"specific comparison to original requirements, expected-result reasoning, and any required corrections"}."#;

pub(crate) fn runs_root(database: &Path) -> PathBuf {
    std::env::var_os("COMMUNITY_CODE_RUNS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            database
                .parent()
                .unwrap_or(Path::new("state"))
                .join("code-runs")
        })
}

pub(crate) fn solve(
    request: &str,
    history: &[Message],
    root: &Path,
    scholar: &mut LocalModel,
    curmudgeon: &mut LocalModel,
    sources: SourceContext,
) -> io::Result<(Report, PathBuf)> {
    let limits = Limits::from_env()?;
    let scholar_id = scholar.model_id()?;
    let reviewer_id = curmudgeon.model_id()?;
    let (log, id) = RunLog::create(root, &limits)?;
    let mut report = new_report(id, request, history, limits, &scholar_id, &reviewer_id)?;
    let generation_tokens = report.limits.generation_tokens;
    report.model_configuration = json!({"scholar":scholar.configuration()?,"curmudgeon":curmudgeon.configuration()?,"coding_sampling":crate::inference::bounded_task_configuration()});
    report.sources = sources;
    log.save(&mut report)?;
    let result = (|| {
        let mut sandbox = sandbox::DockerSandbox::preflight(log.remaining()?)?;
        report.image_id = Some(sandbox.image_id.clone());
        log.save(&mut report)?;
        run_cycle(
            &mut report,
            &log,
            &mut |messages| scholar.chat_for_bounded_task(messages, generation_tokens),
            &mut |messages| curmudgeon.chat_for_bounded_task(messages, generation_tokens),
            &mut |files, tests, budget| sandbox.run(files, tests, budget),
        )
    })();
    if let Err(error) = &result {
        if !report.model_attempts.is_empty() {
            evaluate_failure(&mut report, &log, &error.to_string(), &mut |messages| {
                curmudgeon.chat_for_bounded_task(messages, generation_tokens)
            });
        }
    }
    finish(&mut report, &log, result)?;
    Ok((report, log.directory))
}

pub(crate) fn new_report(
    id: String,
    request: &str,
    history: &[Message],
    limits: Limits,
    scholar: &str,
    curmudgeon: &str,
) -> io::Result<Report> {
    if request.trim().is_empty() || request.len() > 16000 {
        return Err(crate::errors::invalid_data(
            "coding request must contain 1–16000 bytes",
        ));
    }
    let conversation = history.iter().rev().take(12).collect::<Vec<_>>().into_iter().rev()
        .map(|message| json!({"role":message.role,"content":message.content.chars().take(2000).collect::<String>()})).collect();
    Ok(Report {
        protocol: PROTOCOL.into(),
        id,
        request: request.into(),
        conversation,
        scholar_model: scholar.into(),
        curmudgeon_model: curmudgeon.into(),
        model_configuration: serde_json::Value::Null,
        image_id: None,
        sources: SourceContext::default(),
        limits,
        plan: None,
        acceptance_tests: None,
        acceptance_preparation: vec![],
        rounds: vec![],
        model_attempts: vec![],
        failure_evaluation: None,
        failure_evaluation_error: None,
        outcome: Outcome::Unresolved,
        stop_reason: "Run in progress; interrupted runs are unresolved.".into(),
        finished: false,
        elapsed_ms: 0,
    })
}

fn input(report: &Report) -> serde_json::Value {
    json!({"original_request":report.request,"conversation":report.conversation,"limits":report.limits,
        "plan":report.plan,"acceptance_tests":report.acceptance_tests,"source_context":report.sources,
        "acceptance_audit":report.acceptance_preparation.last().map(|preparation| &preparation.audit)})
}

pub(crate) fn run_cycle(
    report: &mut Report,
    log: &RunLog,
    scholar: &mut impl FnMut(&[Message]) -> io::Result<String>,
    curmudgeon: &mut impl FnMut(&[Message]) -> io::Result<String>,
    execute: &mut impl FnMut(&[CodeFile], &str, Duration) -> io::Result<Execution>,
) -> io::Result<()> {
    report.plan = Some(ask_json(
        report,
        log,
        "scholar",
        "hypothesis",
        0,
        messages(PLAN_PROMPT, input(report)),
        scholar,
        Plan::validate,
    )?);
    log.save(report)?;
    let plan = report.plan.clone().expect("validated plan");
    report.sources.required |= plan.requires_external_evidence;
    // Prepare and audit before seeing any implementation. Retain rejected suites;
    // no failing code can cause acceptance tests to be weakened after the fact.
    for _ in 0..2 {
        let draft: AcceptanceDraft = ask_json(
            report,
            log,
            "curmudgeon",
            "acceptance_tests",
            0,
            messages(reviewer::ACCEPTANCE_PROMPT, input(report)),
            curmudgeon,
            |draft: &AcceptanceDraft| {
                reviewer::validate_acceptance(&draft.clone().into_tests()?, &plan)
            },
        )?;
        report.acceptance_tests = Some(draft.into_tests()?);
        log.save(report)?;
        let audit = ask_json(
            report,
            log,
            "scholar",
            "acceptance_audit",
            0,
            messages(TEST_AUDIT_PROMPT, input(report)),
            scholar,
            |audit: &TestAudit| required_text(&audit.reason),
        )
        .unwrap_or_else(|error| TestAudit {
            verdict: TestAuditVerdict::Unresolved,
            reason: error.to_string(),
        });
        let accepted = audit.verdict == TestAuditVerdict::Accept;
        report.acceptance_preparation.push(AcceptancePreparation {
            tests: report.acceptance_tests.clone().expect("validated suite"),
            audit,
        });
        log.save(report)?;
        if accepted {
            break;
        }
    }
    let tests = report
        .acceptance_tests
        .clone()
        .expect("validated acceptance tests");
    for number in 1..=report.limits.rounds {
        log.remaining()?;
        report.outcome = Outcome::Unresolved;
        // Persist the new round before generating: a crashed revision cannot inherit an old success.
        report.rounds.push(Round {
            number,
            candidate: None,
            scholar_execution: None,
            acceptance_execution: None,
            self_assessment: None,
            review: None,
            error: None,
        });
        log.save(report)?;
        crate::diagnostics::event(
            "coding.round_started",
            json!({"run":report.id,"round":number}),
        );
        let result = run_round(
            report, log, number, &plan, &tests, scholar, curmudgeon, execute,
        );
        if let Err(error) = result {
            record_round_error(
                report.rounds.last_mut().expect("current round"),
                &error.to_string(),
            );
            log.save(report)?;
            if number == report.limits.rounds {
                return Err(error);
            }
            continue;
        }
        let review = report
            .rounds
            .last()
            .and_then(|round| round.review.as_ref())
            .expect("reviewed round");
        report.outcome = review.outcome;
        let stop = !review.continue_work;
        if report.outcome == Outcome::Complete || stop {
            report.stop_reason = if report.outcome == Outcome::Complete {
                "Curmudgeon accepted the full requested outcome."
            } else {
                "Curmudgeon stopped with the demonstrated scope and remaining limits recorded."
            }
            .into();
            return Ok(());
        }
        log.save(report)?;
    }
    report.stop_reason =
        "Round limit reached; the last valid Curmudgeon outcome is retained without upgrading it."
            .into();
    Ok(())
}

fn run_round(
    report: &mut Report,
    log: &RunLog,
    number: usize,
    plan: &Plan,
    tests: &AcceptanceTests,
    scholar: &mut impl FnMut(&[Message]) -> io::Result<String>,
    curmudgeon: &mut impl FnMut(&[Message]) -> io::Result<String>,
    execute: &mut impl FnMut(&[CodeFile], &str, Duration) -> io::Result<Execution>,
) -> io::Result<()> {
    let mut data = input(report);
    // Scholar sees the contract; the actual acceptance suite stays with Curmudgeon.
    // This reduces direct example copying, without making a shared model independent.
    data["acceptance_tests"] =
        json!({"rationale":tests.rationale,"covered_requirements":tests.covered_requirements});
    // Keep feedback bounded while preserving all rounds in the durable report.
    data["previous_round"] = serde_json::to_value(report.rounds.iter().rev().nth(1))?;
    data["round"] = json!(number);
    let implementation: Implementation = ask_json(
        report,
        log,
        "scholar",
        "build",
        number,
        messages(BUILD_PROMPT, data),
        scholar,
        Implementation::validate,
    )?;
    let mut candidate = implementation.into_candidate();
    report.rounds.last_mut().unwrap().candidate = Some(candidate.clone());
    log.save(report)?;
    let mut data = input(report);
    data["acceptance_tests"] =
        json!({"rationale":tests.rationale,"covered_requirements":tests.covered_requirements});
    data["implementation"] = serde_json::to_value(&candidate.files)?;
    let generated_tests: io::Result<ScholarTests> = ask_json(
        report,
        log,
        "scholar",
        "test_generation",
        number,
        messages(TESTS_PROMPT, data),
        scholar,
        |tests: &ScholarTests| {
            required_text(&tests.reason)?;
            test_source(&tests.cases).map(|_| ())
        },
    );
    match generated_tests {
        Ok(tests) => {
            candidate.tests = test_source(&tests.cases)?;
            candidate.validate()?;
        }
        Err(error) => record_round_error(
            report.rounds.last_mut().unwrap(),
            &format!("Scholar test generation unresolved: {error}"),
        ),
    }
    report.rounds.last_mut().unwrap().candidate = Some(candidate.clone());
    log.save(report)?;
    for acceptance in [false, true] {
        if !acceptance && candidate.tests.is_empty() {
            continue;
        }
        let budget = log
            .remaining()?
            .min(Duration::from_secs(report.limits.execution_seconds));
        let started = std::time::Instant::now();
        let execution = execute(
            &candidate.files,
            if acceptance {
                &tests.tests
            } else {
                &candidate.tests
            },
            budget,
        )
        .unwrap_or_else(|error| Execution {
            exit_code: None,
            timed_out: error.kind() == io::ErrorKind::TimedOut,
            oom_killed: false,
            output_truncated: false,
            stdout: String::new(),
            stderr: String::new(),
            elapsed_ms: started.elapsed().as_millis(),
            infrastructure_error: Some(error.to_string()),
        });
        let round = report.rounds.last_mut().unwrap();
        if acceptance {
            round.acceptance_execution = Some(execution);
        } else {
            round.scholar_execution = Some(execution);
        }
        log.save(report)?;
    }
    let mut data = input(report);
    data["acceptance_tests"] =
        json!({"rationale":tests.rationale,"covered_requirements":tests.covered_requirements});
    data["current_round"] = serde_json::to_value(report.rounds.last())?;
    let assessment: io::Result<SelfAssessment> = ask_json(
        report,
        log,
        "scholar",
        "self_assessment",
        number,
        messages(ASSESS_PROMPT, data),
        scholar,
        |assessment: &SelfAssessment| {
            required_text(&assessment.reason)?;
            required_text(&assessment.next_change)?;
            text_list(&assessment.limitations)
        },
    );
    match assessment {
        Ok(assessment) => report.rounds.last_mut().unwrap().self_assessment = Some(assessment),
        Err(error) => record_round_error(
            report.rounds.last_mut().unwrap(),
            &format!("Scholar self-assessment unresolved: {error}"),
        ),
    }
    log.save(report)?;
    let mut data = input(report);
    let round = report.rounds.last().unwrap().clone();
    data["current_round"] = serde_json::to_value(&round)?;
    data["previous_round"] = serde_json::to_value(report.rounds.iter().rev().nth(1))?;
    let missing_sources = report.sources.required && report.sources.evidence.is_empty();
    let unaudited_tests = !report
        .acceptance_preparation
        .last()
        .is_some_and(|preparation| preparation.audit.verdict == TestAuditVerdict::Accept);
    let review = ask_json(
        report,
        log,
        "curmudgeon",
        "outcome_review",
        number,
        messages(reviewer::REVIEW_PROMPT, data),
        curmudgeon,
        |review: &Review| {
            if review.outcome == Outcome::Complete && missing_sources {
                return Err(crate::errors::invalid_data(
                    "complete is unavailable: required external source checks have no retained evidence",
                ));
            }
            if review.outcome == Outcome::Complete && unaudited_tests {
                return Err(crate::errors::invalid_data(
                    "complete is unavailable: acceptance test validity remains unresolved",
                ));
            }
            reviewer::validate_review(review, plan, tests, &round)
        },
    )?;
    report.rounds.last_mut().unwrap().review = Some(review);
    log.save(report)
}

fn record_round_error(round: &mut Round, error: &str) {
    round.error = Some(match round.error.take() {
        Some(previous) => format!("{previous}; {error}"),
        None => error.into(),
    });
}

pub(crate) fn finish(report: &mut Report, log: &RunLog, result: io::Result<()>) -> io::Result<()> {
    if let Err(error) = result {
        report.outcome = report
            .failure_evaluation
            .as_ref()
            .map_or(Outcome::Unresolved, |evaluation| evaluation.outcome);
        report.stop_reason =
            format!("Coding cycle could not establish a reviewed outcome: {error}");
    }
    report.finished = true;
    log.save(report)?;
    crate::diagnostics::event(
        "coding.finished",
        json!({"run":report.id,"outcome":report.outcome,
        "rounds":report.rounds.len(),"elapsed_ms":report.elapsed_ms,"stop_reason":report.stop_reason}),
    );
    Ok(())
}

pub(crate) fn evaluate_failure(
    report: &mut Report,
    log: &RunLog,
    error: &str,
    curmudgeon: &mut impl FnMut(&[Message]) -> io::Result<String>,
) {
    let mut data = input(report);
    let last_model_stage = report
        .model_attempts
        .last()
        .map(|attempt| attempt.stage.clone())
        .unwrap_or_else(|| "orchestration".into());
    data["failure"] = json!(error);
    data["last_model_stage"] = json!(last_model_stage);
    data["last_round"] = serde_json::to_value(report.rounds.last()).unwrap_or_default();
    data["recent_failed_stages"] = json!(
        report
            .model_attempts
            .iter()
            .rev()
            .take(3)
            .map(
                |attempt| json!({"role":attempt.role,"stage":attempt.stage,"error":attempt.error,
            "output_excerpt":crate::diagnostics::text_excerpt(&attempt.raw,1200)})
            )
            .collect::<Vec<_>>()
    );
    let result = ask_json(
        report,
        log,
        "curmudgeon",
        "failure_evaluation",
        report.rounds.len(),
        messages(
            "You are Curmudgeon explaining why Scholar's coding cycle could not reach a valid product review. Compare the original request, implementation if any, retained stages and execution results. Copy last_model_stage and failure exactly into the evidence fields. A failure to produce valid structured output is evidence of that stage's failure, not evidence that the user's task is contradictory, impossible or unsupported by the language. Distinguish observed failures from hypotheses about their causes. This provisional failure evaluation cannot approve an artifact or recover a rejected draft as success. Return {\"outcome\":\"not_solved|unresolved\",\"last_model_stage\":\"exact supplied last_model_stage\",\"observed_error\":\"exact supplied failure\",\"reason\":\"what was and was not achieved\",\"causal_analysis\":\"demonstrated failure and clearly labeled causal hypotheses\",\"next_change\":\"concrete next experiment or why to stop\"}.",
            data,
        ),
        curmudgeon,
        |review: &FailureEvaluation| {
            if !matches!(review.outcome, Outcome::NotSolved | Outcome::Unresolved) {
                return Err(crate::errors::invalid_data(
                    "a failure evaluation cannot approve a rejected or unreviewed artifact",
                ));
            }
            if review.last_model_stage != last_model_stage || review.observed_error != error {
                return Err(crate::errors::invalid_data(
                    "failure evaluation must preserve the supplied last stage and observed error exactly",
                ));
            }
            required_text(&review.reason)?;
            required_text(&review.causal_analysis)?;
            required_text(&review.next_change)
        },
    );
    match result {
        Ok(review) => report.failure_evaluation = Some(review),
        Err(error) => report.failure_evaluation_error = Some(error.to_string()),
    }
}

pub(crate) fn load_report(root: &Path, id: &str) -> io::Result<Report> {
    if !id.starts_with("code-")
        || id.len() > 96
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(crate::errors::invalid_data("invalid coding run ID"));
    }
    let bytes = std::fs::read(root.join(id).join("report.json"))?;
    let mut report: Report = serde_json::from_slice(&bytes).map_err(crate::errors::invalid_json)?;
    if report.protocol != PROTOCOL || report.id != id {
        return Err(crate::errors::invalid_data(
            "coding run identity/version mismatch",
        ));
    }
    if !report.finished {
        report.outcome = Outcome::Unresolved;
    }
    Ok(report)
}

pub(crate) fn export(report: &Report, destination: &Path) -> io::Result<()> {
    use std::fs;
    use std::io::Write;
    let candidate = report
        .rounds
        .last()
        .and_then(|round| round.candidate.as_ref())
        .ok_or_else(|| crate::errors::invalid_data("coding run has no final artifact"))?;
    candidate.validate_implementation()?;
    if !candidate.tests.is_empty() {
        validate_test_code(&candidate.tests)?;
    }
    // Never overwrite or merge into a checkout: export to a new user-selected directory.
    fs::create_dir(destination)?;
    for file in &candidate.files {
        let path = destination.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        output.write_all(file.content.as_bytes())?;
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination.join("test_scholar.py"))?
        .write_all(candidate.tests.as_bytes())?;
    if let Some(tests) = &report.acceptance_tests {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination.join("test_acceptance.py"))?
            .write_all(tests.tests.as_bytes())?;
    }
    Ok(())
}
