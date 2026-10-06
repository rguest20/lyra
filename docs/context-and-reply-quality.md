# Conversation context and reply quality fixes

- [x] Exclude common words from editable map relevance, using the same filter as SQLite retrieval.
- [x] Prevent `thank` from matching the common relation word `than` through prefix matching.
- [x] Log the exact selected map, curated claim and Scholar finding IDs for each reply.
- [x] Tell Lyra to answer directly and acknowledge thanks briefly.
- [x] Replace the canned uncertainty preamble with instructions to identify missing evidence.
- [x] Give narration, echoing, role reversal and stock invitations one automated rewrite without thinking.
- [x] Review answers based solely on unpromoted research with an AI grounding pass and one bounded rewrite/review cycle.
- [x] Label partial thinking from a discarded generation attempt.
- [x] Add regression tests and replay copies of the screenshot's conversation.

## What went wrong

The editable knowledge map counted words such as `the` as relevance. A question
about an element could therefore select the unrelated Hagia Sophia claim.
After excluding common words, the prefix matcher also revealed a `thank` / `than`
collision. Both are covered by regressions. Genuine queries about Hagia Sophia
still retrieve the claim.

Lyra sometimes narrated the user's request and the source assessment rather than
answering the user. It also attributed its own honesty to the user and appended
offers of further help despite the existing prompt. The first live replay showed
that cleaning up style alone could produce an uncited discoverer name from model
memory. Answers relying solely on unpromoted Scholar passages now receive a
separate AI evidence review before they are returned.

The review allows equivalent meanings, preserves uncertainty and distinguishes
missing evidence from a false assertion. It accepts an honest acknowledgment that
a passage does not supply the requested fact. A rejected draft gets one rewrite
and a second review. Missing source links on that rewrite receive a `Sources
checked` list from the supplied metadata. Repeated rejection returns a brief
acknowledgment that no grounded answer is available. Invalid review output
produces a logged error, without a human review queue.

## Logs

- `conversation.context_selected`: question, conversation ID, selected map and
  curated claim IDs, Scholar finding IDs, pending observation count.
- `conversation.reply_repair`: reason and initial draft excerpt.
- `conversation.reply_repaired`: reason and revised excerpt.
- `conversation.reply_repair_failed`: reason and unsuitable revised excerpt.
- `conversation.research_reply_review`: attempt, supported decision, reason,
  draft excerpt and source URLs.
- `conversation.research_reply_review_invalid`: schema failure and raw output excerpt.
- `conversation.research_reply_unresolved`: exhausted review attempts and the
  automatic acknowledgment action.

Browser reply diagnostics are forwarded to server stderr. With the command below,
they appear in `community-lab.log`.

```sh
cargo run --release -- serve 2>>community-lab.log
```

Stop the existing server before restarting it. Saved conversations are preserved.

## Validation and limits

229 unit tests pass and the release build succeeds. Regression coverage includes
the actual prompt selection, screenshot reply patterns, Unicode cleanup, bounded
grounding repair and invalid review responses. Live replay uses copies of the
session, knowledge map and SQLite database, preserving the real conversation.
Replay artifacts are under `state/context-reply-checks/20261005-215820`.

The live thank-you replay returned “You're welcome! I appreciate your feedback.”
Element-question replays confirmed empty map claim IDs, rejected guessed
discoverer names, and exposed a reviewer hallucination about the supplied
passages. The bounded fallback avoids publishing rejected drafts even when the
reviewer cannot assess a valid missing-evidence acknowledgment reliably.
The final element replay completed successfully with “I don't have a grounded
answer to that yet,” logging both rejected drafts and the exhausted review action.

The grounding review uses the same Qwen3.5-2B model and is not a guarantee of
correctness. It adds generation latency. This pass applies when Scholar evidence
is supplied without relevant active map or curated claims; those other answer
paths retain their existing provenance and challenge rules. These changes do not
improve Scholar's selection of passages or find a missing discoverer's name.
