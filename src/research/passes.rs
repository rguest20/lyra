//! Bounded structured model passes: ask, validate, and retry with a repair
//! prompt a fixed number of times. Validation stays with the caller so an
//! invalid or uncertain output is never silently accepted.
use super::*;

/// Ask the model up to `attempts` times until `parse` accepts the output.
///
/// `parse` receives the 1-based attempt number so callers can record which
/// pass produced a verdict. After every failure except the last, `repair`
/// builds the next prompt from the current messages, the raw output and the
/// validation error; the final failure is returned unchanged.
pub(crate) fn structured_pass<T>(
    mut messages: Vec<Message>,
    attempts: usize,
    mut ask: impl FnMut(&[Message]) -> io::Result<String>,
    mut parse: impl FnMut(usize, &str) -> io::Result<T>,
    mut repair: impl FnMut(Vec<Message>, String, &io::Error) -> Vec<Message>,
) -> io::Result<T> {
    assert!(
        attempts >= 1,
        "a structured pass needs at least one attempt"
    );
    for attempt in 1..=attempts {
        let raw = ask(&messages)?;
        match parse(attempt, &raw) {
            Ok(value) => return Ok(value),
            Err(error) if attempt < attempts => messages = repair(messages, raw, &error),
            Err(error) => return Err(error),
        }
    }
    unreachable!("the final attempt either returns a value or its error")
}

/// Keep the conversation and append the rejected output with a correction
/// request, so the next pass reconsiders the same request with the same context.
pub(crate) fn append_repair(
    mut messages: Vec<Message>,
    raw: String,
    instruction: String,
) -> Vec<Message> {
    messages.push(Message {
        role: "assistant".into(),
        content: raw,
    });
    messages.push(Message {
        role: "user".into(),
        content: instruction,
    });
    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn system(content: &str) -> Vec<Message> {
        vec![Message {
            role: "system".into(),
            content: content.into(),
        }]
    }

    #[test]
    fn returns_first_valid_output_without_repairing() {
        let calls = RefCell::new(0);
        let value = structured_pass(
            system("s"),
            2,
            |_| {
                *calls.borrow_mut() += 1;
                Ok("ok".into())
            },
            |attempt, raw| {
                assert_eq!(attempt, 1);
                Ok(raw.to_owned())
            },
            |_, _, _| panic!("repair must not run after a valid output"),
        )
        .unwrap();
        assert_eq!(value, "ok");
        assert_eq!(*calls.borrow(), 1);
    }

    #[test]
    fn repairs_once_then_returns_the_final_error() {
        let prompts = RefCell::new(Vec::new());
        let error = structured_pass::<String>(
            system("s"),
            2,
            |messages| {
                prompts.borrow_mut().push(messages.len());
                Ok("bad".into())
            },
            |_, _| Err(invalid("not json")),
            |messages, raw, error| {
                assert_eq!(raw, "bad");
                assert_eq!(error.to_string(), "not json");
                append_repair(messages, raw, "fix it".into())
            },
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "not json");
        assert_eq!(*prompts.borrow(), vec![1, 3]);
    }

    #[test]
    fn second_attempt_sees_the_repaired_prompt_and_its_attempt_number() {
        let value = structured_pass(
            system("s"),
            3,
            |messages| Ok(format!("{}", messages.len())),
            |attempt, raw| {
                if attempt == 2 {
                    Ok(raw.to_owned())
                } else {
                    Err(invalid("retry"))
                }
            },
            |messages, raw, _| append_repair(messages, raw, "again".into()),
        )
        .unwrap();
        assert_eq!(value, "3");
    }

    #[test]
    fn transport_errors_stop_immediately() {
        let error = structured_pass(
            system("s"),
            3,
            |_| Err(io::Error::other("worker gone")),
            |_, raw: &str| Ok(raw.to_owned()),
            |_, _, _| panic!("no repair for transport failures"),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "worker gone");
    }
}
