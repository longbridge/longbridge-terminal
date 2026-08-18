//! Chat and continue command orchestration.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use rust_i18n::t;
use serde_json::{json, Value};

use super::client::{stream_conversation, ConversationRequest};
use super::events::{AgentEvent, ChatAggregator, ChatOutcome};
use super::ChatTarget;
use crate::ai::stdout::render_answer;
use crate::cli::OutputFormat;
use crate::utils::text::strip_control_chars;

/// Single-quote `s` for safe inclusion in a copy-pasteable POSIX shell
/// command line, escaping embedded single quotes as `'\''`. Used for
/// server/LLM-controlled text (interrupt questions) that gets echoed back
/// into a `longbridge agent continue --answer "..."` hint: unescaped, a
/// question containing `"`, `$(...)`, or backticks would break out of a
/// double-quoted argument and inject shell commands into the copy-pasted
/// line.
fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Quote a value for a copy-pasteable shell command line: strip control
/// characters (terminal escapes), then single-quote. Applied to every
/// server-supplied argument (`agent_uid`, `chat_uid`, `message_id`, questions)
/// that the CLI prints inside a suggested command.
fn shell_arg(s: &str) -> String {
    shell_single_quote(&strip_control_chars(s))
}

// ── answers_by_tool_call assembly ───────────────────────────────────────────

/// All `(tool_call_id, question)` pairs a cached interrupt knows about.
fn cached_question_pairs(interrupt: &Value) -> Vec<(String, String)> {
    let Some(tool_call_id) = interrupt.get("tool_call_id").and_then(Value::as_str) else {
        return Vec::new();
    };
    interrupt
        .get("questions")
        .and_then(Value::as_array)
        .map(|qs| {
            qs.iter()
                .filter_map(|q| q.get("question").and_then(Value::as_str))
                .map(|q| (tool_call_id.to_string(), q.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve a full-form `--answer` against the questions the cached interrupt
/// actually asked, by exact `tool_call_id:question=` prefix match.
///
/// The naive first-`=` split mis-parses any question containing `=` (e.g.
/// "Is P/E=20 acceptable?"): it would cut the key at the wrong place and send
/// the server an answer keyed by a question it never asked. Matching against
/// the known pairs removes the ambiguity; the longest match wins so a question
/// that is a prefix of another cannot shadow it.
fn resolve_against_cache(spec: &str, interrupt: &Value) -> Option<(String, String, String)> {
    cached_question_pairs(interrupt)
        .into_iter()
        .filter_map(|(tool_call_id, question)| {
            let prefix = format!("{tool_call_id}:{question}=");
            spec.strip_prefix(&prefix)
                .map(|answer| (prefix.len(), tool_call_id, question, answer.to_string()))
        })
        .max_by_key(|(len, ..)| *len)
        .map(|(_, tool_call_id, question, answer)| (tool_call_id, question, answer))
}

/// Build the continue request body from `--answer` values.
///
/// Full form: `tool_call_id:question=answer`. Bare form (no `:`/`=` prefix
/// structure) resolves against the cached interrupt when it has exactly one
/// question.
pub(crate) fn parse_answer_specs(
    answers: &[String],
    cached_interrupt: Option<&Value>,
) -> Result<Value> {
    let mut by_tool_call: HashMap<String, HashMap<String, String>> = HashMap::new();
    for spec in answers {
        // Prefer an exact match against the questions actually asked, so a
        // question containing `=` still round-trips.
        if let Some((tool_call_id, question, answer)) =
            cached_interrupt.and_then(|i| resolve_against_cache(spec, i))
        {
            by_tool_call
                .entry(tool_call_id)
                .or_default()
                .insert(question, answer);
            continue;
        }
        if let Some((prefix, answer)) = spec.split_once('=') {
            if let Some((tool_call_id, question)) = prefix.split_once(':') {
                by_tool_call
                    .entry(tool_call_id.to_string())
                    .or_default()
                    .insert(question.to_string(), answer.to_string());
                continue;
            }
        }
        // Bare form: needs the cached interrupt with a single question
        let Some(interrupt) = cached_interrupt else {
            bail!(
                "A bare --answer needs a locally cached interrupt; use the full form\n\
                 --answer \"<tool_call_id>:<question>=<answer>\" (printed by the chat command)"
            );
        };
        let tool_call_id = interrupt
            .get("tool_call_id")
            .and_then(Value::as_str)
            .context("Cached interrupt has no tool_call_id")?;
        let questions = interrupt
            .get("questions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if questions.len() != 1 {
            bail!(
                "The interrupted run asked {} questions; use the full form \
                 --answer \"<tool_call_id>:<question>=<answer>\" for each",
                questions.len()
            );
        }
        let question = questions[0]
            .get("question")
            .and_then(Value::as_str)
            .context("Cached interrupt question is malformed")?;
        by_tool_call
            .entry(tool_call_id.to_string())
            .or_default()
            .insert(question.to_string(), spec.clone());
    }
    if by_tool_call.is_empty() {
        bail!("No answers given; pass --answer or --interactive");
    }
    Ok(serde_json::to_value(by_tool_call)?)
}

// ── interrupt cache ─────────────────────────────────────────────────────────

fn interrupt_cache_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".longbridge")
        .join("openapi")
        .join("ai_interrupts")
}

/// Sanitize a server-supplied ID for use as a filename component: only
/// `[A-Za-z0-9_-]` survive, everything else (notably `/` and `.`) becomes
/// `_`. Without this, a malicious/buggy `chat_uid` like `../../../tmp/x`
/// would let `Path::join` escape `dir` entirely.
fn sanitize_cache_component(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Full 64-bit digest of the raw `chat_uid\0message_id` tuple.
///
/// Truncating to 32 bits was not enough: a probe found real id-shaped inputs
/// that collide in 32 bits, which would silently swap one conversation's
/// cached interrupt for another's. The digest is a cache key only — a
/// toolchain change may alter `DefaultHasher` and orphan old files, which
/// costs at most one `--answer` full-form round.
fn cache_digest(chat_uid: &str, message_id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut raw = Vec::with_capacity(chat_uid.len() + message_id.len() + 1);
    raw.extend_from_slice(chat_uid.as_bytes());
    raw.push(0);
    raw.extend_from_slice(message_id.as_bytes());
    raw.hash(&mut hasher);
    hasher.finish()
}

/// Cache filename for one interrupted round.
///
/// `sanitize_cache_component` is many-to-one (`a/b` and `a.b` both become
/// `a_b`), so the sanitized names alone would let two distinct conversations
/// clobber each other's cached interrupt. A digest of the *raw* ids
/// disambiguates them while the sanitized part keeps the name readable.
fn cache_file(dir: &Path, chat_uid: &str, message_id: &str) -> PathBuf {
    let digest = cache_digest(chat_uid, message_id);
    dir.join(format!(
        "{}-{}-{digest:016x}.json",
        sanitize_cache_component(chat_uid),
        sanitize_cache_component(message_id)
    ))
}

pub(crate) fn save_interrupt_in(
    dir: &Path,
    chat_uid: &str,
    message_id: &str,
    interrupt: &Value,
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(
        cache_file(dir, chat_uid, message_id),
        serde_json::to_string(interrupt)?,
    )?;
    Ok(())
}

pub(crate) fn load_interrupt_in(dir: &Path, chat_uid: &str, message_id: &str) -> Option<Value> {
    let content = std::fs::read_to_string(cache_file(dir, chat_uid, message_id)).ok()?;
    serde_json::from_str(&content).ok()
}

pub(crate) fn clear_interrupt_in(dir: &Path, chat_uid: &str, message_id: &str) {
    let _ = std::fs::remove_file(cache_file(dir, chat_uid, message_id));
}

fn save_interrupt(chat_uid: &str, message_id: &str, interrupt: &Value) -> Result<()> {
    save_interrupt_in(&interrupt_cache_dir(), chat_uid, message_id, interrupt)
}

fn load_interrupt(chat_uid: &str, message_id: &str) -> Option<Value> {
    load_interrupt_in(&interrupt_cache_dir(), chat_uid, message_id)
}

fn clear_interrupt(chat_uid: &str, message_id: &str) {
    clear_interrupt_in(&interrupt_cache_dir(), chat_uid, message_id);
}

/// Fill `outcome.chat_uid` / `outcome.message_id` from the known request IDs
/// when the SSE stream never sent a `chat_started` event to populate them
/// (observed on `/continue`, which resumes an existing conversation and may
/// skip re-announcing its identity). Only touches fields that are empty, so
/// a real `chat_started` id always wins.
fn backfill_outcome_ids(outcome: &mut ChatOutcome, chat_uid: &str, message_id: &str) {
    if outcome.chat_uid.is_empty() {
        outcome.chat_uid = chat_uid.to_string();
    }
    if outcome.message_id.is_empty() {
        outcome.message_id = message_id.to_string();
    }
}

// ── hint strings ────────────────────────────────────────────────────────────

/// A copy-pasteable `agent continue` command with the full --answer form
/// prefilled (answer value left for the user to fill in).
pub(crate) fn continue_hint(
    agent_uid: &str,
    chat_uid: &str,
    message_id: &str,
    interrupt: &Value,
) -> String {
    let tool_call_id = strip_control_chars(
        interrupt
            .get("tool_call_id")
            .and_then(Value::as_str)
            .unwrap_or("<tool_call_id>"),
    );
    let mut lines = Vec::new();
    let empty = Vec::new();
    let questions = interrupt
        .get("questions")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    for q in questions {
        let question =
            strip_control_chars(q.get("question").and_then(Value::as_str).unwrap_or("?"));
        let choices: Vec<String> = question_choices(q);
        let choice_note = if choices.is_empty() {
            String::new()
        } else {
            format!("   ({})", choices.join(" / "))
        };
        lines.push(format!("  [{tool_call_id}] {question}{choice_note}"));
    }
    // Question text is server/LLM-controlled: single-quote the whole
    // `tool_call_id:question=<answer>` payload so `"`, `$( )`, and
    // backticks in a hostile question cannot break out of the argument or
    // inject shell commands when this hint is copy-pasted.
    let mut answers = String::new();
    for q in questions {
        let question =
            strip_control_chars(q.get("question").and_then(Value::as_str).unwrap_or("?"));
        let payload = format!("{tool_call_id}:{question}=<answer>");
        let _ = write!(
            answers,
            " \\\n    --answer {}",
            shell_single_quote(&payload)
        );
    }
    // The ids are server-supplied too: a hostile `chat_uid` containing
    // `$(...)`/backticks would otherwise execute when this line is pasted.
    format!(
        "{}\nlongbridge agent continue {} {} {}{answers}",
        lines.join("\n"),
        shell_arg(agent_uid),
        shell_arg(chat_uid),
        shell_arg(message_id),
    )
}

// ── chat / continue orchestration ───────────────────────────────────────────

pub async fn cmd_chat(
    target: ChatTarget,
    stream: bool,
    interactive: bool,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    // `chat --interactive` prompts on the terminal exactly like `continue`
    // does when the agent asks something back, so it carries the same
    // pretty-only constraint. Check before any network work.
    ensure_interactive_supported(interactive, format)?;
    let req = ConversationRequest::New {
        agent_uid: target.agent_uid.clone(),
        query: target.query.clone(),
        chat_uid: target.chat_uid.clone(),
        parent_message_id: target.parent_message_id.clone(),
    };
    let outcome = run_streaming(req, stream, format, verbose).await?;
    handle_outcome(
        &target.agent_uid,
        outcome,
        stream,
        interactive,
        format,
        verbose,
    )
    .await
}

/// The selectable options of one interrupt question, already sanitized.
///
/// The server sends `options: [{description}]`. An earlier reading of the
/// payload assumed a bare `choices: [string]`, which silently produced an
/// empty option list: the prompt showed the question with no choices, and
/// answering `1` sent the literal "1" instead of the option text. Both shapes
/// are accepted so neither a rollback nor a future tweak can blank the prompt
/// again.
pub(crate) fn question_choices(q: &Value) -> Vec<String> {
    if let Some(options) = q.get("options").and_then(Value::as_array) {
        let from_options: Vec<String> = options
            .iter()
            .filter_map(|o| {
                o.get("description")
                    .and_then(Value::as_str)
                    .or_else(|| o.as_str())
            })
            .map(strip_control_chars)
            .collect();
        if !from_options.is_empty() {
            return from_options;
        }
    }
    q.get("choices")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(strip_control_chars)
                .collect()
        })
        .unwrap_or_default()
}

/// `--interactive` prompts on the terminal, which is meaningless (and
/// output-corrupting) for a machine-readable run: with `--format json` the
/// caller is a script that cannot answer. Fail before prompting instead of
/// blocking on stdin.
pub(crate) fn ensure_interactive_supported(interactive: bool, format: &OutputFormat) -> Result<()> {
    if interactive && matches!(format, OutputFormat::Json) {
        bail!(
            "--interactive requires the default pretty output; with --format json pass the \
             answers up front: --answer \"<tool_call_id>:<question>=<answer>\""
        );
    }
    Ok(())
}

/// Same check, driven straight off the parsed subcommand so `main` can run it
/// before any network work. `chat` and `continue` both take `--interactive`;
/// `list` does not.
pub(crate) fn ensure_interactive_supported_for(
    cmd: &crate::cli::AgentCmd,
    format: &OutputFormat,
) -> Result<()> {
    use crate::cli::AgentCmd;
    let interactive = match cmd {
        AgentCmd::Chat { interactive, .. } | AgentCmd::Continue { interactive, .. } => *interactive,
        AgentCmd::List { .. }
        | AgentCmd::Workspaces
        | AgentCmd::Chats { .. }
        | AgentCmd::ChatDetail { .. } => false,
    };
    ensure_interactive_supported(interactive, format)
}

/// Validate a raw `--answers-json` payload: it must be an object of objects
/// of strings, i.e. exactly the `answers_by_tool_call` shape the API expects.
///
/// This is the unambiguous escape hatch from `--answer`'s
/// `tool_call_id:question=answer` grammar — no separator can be confused for
/// content, whatever the question contains.
/// Every value echoed back by [`parse_answers_json`] is attacker-controlled:
/// an AI harness drives `--answers-json`, and its keys travel unmodified into
/// stderr. Strip terminal control sequences and cap the length so a hostile
/// payload cannot repaint the user's terminal through a validation error.
fn sanitize_for_error(s: &str) -> String {
    const MAX: usize = 120;
    let clipped: String = s.chars().take(MAX).collect();
    let mut out = strip_control_chars(&clipped);
    if s.chars().nth(MAX).is_some() {
        out.push('…');
    }
    out
}

pub(crate) fn parse_answers_json(raw: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(raw).with_context(|| {
        format!(
            "--answers-json is not valid JSON: {}",
            sanitize_for_error(raw)
        )
    })?;
    let Some(by_tool_call) = value.as_object() else {
        bail!(
            "--answers-json must be a JSON object keyed by tool_call_id, e.g. \
             '{{\"call_a\":{{\"Which period?\":\"1m\"}}}}'"
        );
    };
    if by_tool_call.is_empty() {
        bail!("--answers-json is empty; nothing to answer");
    }
    for (tool_call_id, answers) in by_tool_call {
        let tool_call_id = sanitize_for_error(tool_call_id);
        let Some(answers) = answers.as_object() else {
            bail!("--answers-json: value for \"{tool_call_id}\" must be an object of question -> answer");
        };
        if answers.is_empty() {
            bail!("--answers-json: \"{tool_call_id}\" has no answers");
        }
        for (question, answer) in answers {
            if !answer.is_string() {
                let question = sanitize_for_error(question);
                bail!(
                    "--answers-json: answer for \"{tool_call_id}\" / \"{question}\" must be a string"
                );
            }
        }
    }
    Ok(value)
}

/// Resolve the `answers_by_tool_call` payload for `continue` from the three
/// mutually exclusive input forms, doing every check that can fail *before*
/// any request is built.
///
/// The pretty-only interactive guard runs first, ahead of the branch, so
/// `--answers-json` cannot smuggle `--interactive --format json` past it —
/// that ordering is the whole point of keeping this as one pure step.
pub(crate) fn resolve_continue_answers(
    chat_uid: &str,
    message_id: &str,
    answers: &[String],
    answers_json: Option<&str>,
    interactive: bool,
    format: &OutputFormat,
) -> Result<Value> {
    ensure_interactive_supported(interactive, format)?;
    if let Some(raw) = answers_json {
        // Explicit payload wins outright: nothing to prompt for, nothing to parse.
        return parse_answers_json(raw);
    }
    let cached = load_interrupt(chat_uid, message_id);
    if answers.is_empty() && interactive {
        let interrupt = cached
            .context("No cached interrupt found; pass --answer with the full form instead")?;
        prompt_answers(&interrupt)
    } else {
        parse_answer_specs(answers, cached.as_ref())
    }
}

pub async fn cmd_continue(
    agent_uid: String,
    chat_uid: String,
    message_id: String,
    answers: Vec<String>,
    answers_json: Option<String>,
    interactive: bool,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let answers_value = resolve_continue_answers(
        &chat_uid,
        &message_id,
        &answers,
        answers_json.as_deref(),
        interactive,
        format,
    )?;
    run_continue(
        agent_uid,
        chat_uid,
        message_id,
        answers_value,
        interactive,
        format,
        verbose,
    )
    .await
}

/// POST the resolved answers to the continue endpoint and present the result.
async fn run_continue(
    agent_uid: String,
    chat_uid: String,
    message_id: String,
    answers_value: Value,
    interactive: bool,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let req = ConversationRequest::Continue {
        agent_uid: agent_uid.clone(),
        chat_uid: chat_uid.clone(),
        message_id: message_id.clone(),
        answers: answers_by_tool_call(answers_value)?,
    };
    let mut outcome = run_streaming(req, false, format, verbose).await?;
    backfill_outcome_ids(&mut outcome, &chat_uid, &message_id);
    clear_interrupt(&chat_uid, &message_id);
    handle_outcome(&agent_uid, outcome, false, interactive, format, verbose).await
}

/// Convert the validated `answers_by_tool_call` JSON (an object of objects of
/// strings) into the SDK's typed map. The shape was already checked upstream
/// (`parse_answers_json` / `parse_answer_specs` / `prompt_answers`), so a
/// failure here means a programming error rather than bad user input.
fn answers_by_tool_call(value: Value) -> Result<longbridge::agent::AnswersByToolCall> {
    serde_json::from_value(value).context("Invalid answers payload")
}

/// Drive the SSE stream, printing progress to stderr (pretty) and answer
/// deltas to stdout (--stream). Returns the aggregated outcome.
async fn run_streaming(
    req: ConversationRequest,
    stream: bool,
    format: &OutputFormat,
    verbose: bool,
) -> Result<ChatOutcome> {
    let pretty = matches!(format, OutputFormat::Pretty);
    let mut agg = ChatAggregator::default();
    let mut answer_started = false;
    let result = stream_conversation(req, verbose, &mut |ev| {
        if pretty {
            match &ev {
                AgentEvent::ThinkingStarted => eprintln!("* {}", t!("Agent.Thinking")),
                AgentEvent::ToolUseStarted { tool_name } => {
                    let tool_name = strip_control_chars(tool_name);
                    eprintln!("* {}", t!("Agent.CallingTool", name = tool_name));
                }
                AgentEvent::ToolUseFinished {
                    tool_name, status, ..
                } => {
                    let tool_name = strip_control_chars(tool_name);
                    let status = strip_control_chars(status);
                    eprintln!(
                        "* {}",
                        t!("Agent.ToolDone", name = tool_name, status = status)
                    );
                }
                AgentEvent::AnswerDelta { text } => {
                    if !answer_started {
                        answer_started = true;
                        eprintln!("* {}", t!("Agent.Generating"));
                    }
                    if stream {
                        print!("{}", strip_control_chars(text));
                        let _ = std::io::stdout().flush();
                    }
                }
                _ => {}
            }
        }
        agg.push(&ev);
    })
    .await;
    let outcome = agg.finish();
    match result {
        Ok(()) => Ok(outcome),
        Err(e) if !outcome.answer.is_empty() => {
            // Partial answer: warn but still show what we have
            // `e` wraps server-controlled stream text; sanitize before stderr.
            eprintln!(
                "{} ({})",
                t!("Agent.PartialAnswer"),
                strip_control_chars(&format!("{e:#}"))
            );
            Ok(outcome)
        }
        Err(e) => Err(e),
    }
}

/// Hard cap on inline interactive resume rounds. A hostile/buggy server that
/// replies `interrupted` forever, or a user piping empty stdin, must not
/// drive unbounded recursion; once the cap is hit we fall through to the
/// normal `interrupted` output (which prints the continue hint) instead of
/// resuming again.
const MAX_INTERACTIVE_ROUNDS: u32 = 5;

async fn handle_outcome(
    agent_uid: &str,
    outcome: ChatOutcome,
    streamed: bool,
    interactive: bool,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    handle_outcome_depth(
        agent_uid,
        outcome,
        streamed,
        interactive,
        format,
        verbose,
        0,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn handle_outcome_depth(
    agent_uid: &str,
    outcome: ChatOutcome,
    streamed: bool,
    interactive: bool,
    format: &OutputFormat,
    verbose: bool,
    depth: u32,
) -> Result<()> {
    // Interactive resume: answer the questions inline and immediately continue.
    if outcome.status == "interrupted" {
        if let Some(interrupt) = &outcome.interrupt {
            // Best-effort: a local fs failure (e.g. permissions, full disk)
            // must not suppress the interrupted output or flip the exit
            // code — the interrupt is still shown/returned to the caller
            // via the printed hint / JSON `interrupt` field either way, a
            // bare `--answer` resume just won't be available.
            if let Err(e) = save_interrupt(&outcome.chat_uid, &outcome.message_id, interrupt) {
                eprintln!("Warning: failed to cache interrupt locally: {e}");
            }
            if interactive
                && matches!(format, OutputFormat::Pretty)
                && depth < MAX_INTERACTIVE_ROUNDS
            {
                eprintln!("{}", t!("Agent.Interrupted"));
                let answers = prompt_answers(interrupt)?;
                let req = ConversationRequest::Continue {
                    agent_uid: agent_uid.to_string(),
                    chat_uid: outcome.chat_uid.clone(),
                    message_id: outcome.message_id.clone(),
                    answers: answers_by_tool_call(answers)?,
                };
                let mut next = run_streaming(req, streamed, format, verbose).await?;
                backfill_outcome_ids(&mut next, &outcome.chat_uid, &outcome.message_id);
                clear_interrupt(&outcome.chat_uid, &outcome.message_id);
                return Box::pin(handle_outcome_depth(
                    agent_uid,
                    next,
                    streamed,
                    interactive,
                    format,
                    verbose,
                    depth + 1,
                ))
                .await;
            }
        }
    }

    let verdict = classify_outcome(&outcome);
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&outcome)?);
            // The JSON object above still carries whatever partial answer was
            // received; a non-success verdict must still exit non-zero.
            if let Verdict::Failure(msg) = verdict {
                bail!("{msg}");
            }
        }
        OutputFormat::Pretty => match verdict {
            Verdict::Interrupted => {
                let hint = outcome
                    .interrupt
                    .as_ref()
                    .map(|i| continue_hint(agent_uid, &outcome.chat_uid, &outcome.message_id, i))
                    .unwrap_or_default();
                println!("{}", t!("Agent.Interrupted"));
                println!("{hint}");
            }
            Verdict::Success => {
                render_pretty_answer(&outcome, streamed).await;
                print_footer(agent_uid, &outcome);
            }
            Verdict::Failure(msg) => {
                // Print whatever is printable (a partial answer and the ids
                // needed to resume), then fail.
                if !outcome.answer.is_empty() {
                    render_pretty_answer(&outcome, streamed).await;
                    print_footer(agent_uid, &outcome);
                }
                bail!("{msg}");
            }
        },
    }
    Ok(())
}

/// Terminal verdict for a finished round. Deliberately an allowlist: only an
/// explicit `succeeded`, or an `interrupted` that actually carries the
/// interrupt payload the user needs to resume, count as a clean exit. Every
/// other status — `failed`, `stopped`, `cancelled`, an empty status, a novel
/// one a future server introduces, or an `interrupted` with no payload — is a
/// failure, so a run that did not produce an answer can never exit 0.
#[derive(Debug, PartialEq)]
enum Verdict {
    Success,
    Interrupted,
    Failure(String),
}

fn classify_outcome(outcome: &ChatOutcome) -> Verdict {
    // `status` and `error_message` are server-supplied and end up on stderr
    // verbatim through the bail; sanitize before they are interpolated.
    let status = strip_control_chars(&outcome.status);
    let error_message = strip_control_chars(&outcome.error_message);
    match status.as_str() {
        "succeeded" => Verdict::Success,
        "interrupted" if outcome.interrupt.is_some() => Verdict::Interrupted,
        "interrupted" => Verdict::Failure(
            "Agent run interrupted but the server sent no interrupt payload; \
             nothing to answer -- retry the request"
                .to_string(),
        ),
        // "unknown" means the SSE stream was dropped mid-run (see
        // `ChatAggregator::finish`).
        "unknown" => Verdict::Failure(t!("Agent.StreamDropped").to_string()),
        "" => Verdict::Failure(
            "Agent run ended without a status; the run did not complete".to_string(),
        ),
        other if error_message.is_empty() => {
            Verdict::Failure(format!("Agent run did not succeed (status={other})"))
        }
        other => Verdict::Failure(format!("Agent run {other}: {error_message}")),
    }
}

/// Render the answer body for pretty output (skipped when it was already
/// streamed to stdout token by token).
async fn render_pretty_answer(outcome: &ChatOutcome, streamed: bool) {
    if streamed {
        return;
    }
    let quotes = crate::ai::quotes::fetch_cards(&outcome.widgets).await;
    let width = crossterm::terminal::size().map_or(80, |(w, _)| w as usize);
    let color = std::io::IsTerminal::is_terminal(&std::io::stdout());
    print!("{}", render_answer(&outcome.answer, &quotes, width, color));
}

fn print_footer(agent_uid: &str, outcome: &ChatOutcome) {
    if !outcome.references.is_empty() {
        println!("{}:", t!("Agent.References"));
        for r in &outcome.references {
            let idx = r.index;
            let content = r.content.clone().unwrap_or(Value::Null);
            // News-article `content` shape.
            let source =
                strip_control_chars(content.get("source").and_then(Value::as_str).unwrap_or(""));
            let desc = strip_control_chars(
                content
                    .get("description")
                    .and_then(Value::as_str)
                    .or_else(|| content.get("title").and_then(Value::as_str))
                    .unwrap_or(""),
            );
            if source.is_empty() && desc.is_empty() {
                // Non-news reference (e.g. a `SecurityQuote` whose `content`
                // has no source/description) — fall back to the identity the
                // server did send so the line isn't blank.
                let ty = strip_control_chars(&r.ref_type);
                let id = strip_control_chars(&r.id);
                let label = [ty.as_str(), id.as_str()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ");
                println!("  [{idx}] {label}");
            } else {
                let published = strip_control_chars(
                    content
                        .get("published_at")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                );
                println!("  [{idx}] {source} · {desc} {published}");
            }
        }
    }
    if !outcome.further_questions.is_empty() {
        println!("{}:", t!("Agent.FurtherQuestions"));
        for q in &outcome.further_questions {
            println!("  · {}", strip_control_chars(q));
        }
    }
    println!("─────");
    let elapsed = outcome
        .elapsed_time
        .map(|s| t!("Agent.Elapsed", secs = format!("{s:.1}")).to_string())
        .unwrap_or_default();
    println!(
        "chat_uid: {} · message_id: {} · {elapsed}",
        strip_control_chars(&outcome.chat_uid),
        strip_control_chars(&outcome.message_id)
    );
    // Same treatment as `continue_hint`: this line is meant to be pasted into
    // a shell, so every server-supplied argument is stripped and quoted.
    println!(
        "{} longbridge agent chat {} {} {} \"<query>\"",
        t!("Agent.FollowUpWith"),
        shell_arg(agent_uid),
        shell_arg(&outcome.chat_uid),
        shell_arg(&outcome.message_id)
    );
}

/// Prompt for each question on the terminal (numbered choices accepted).
fn prompt_answers(interrupt: &Value) -> Result<Value> {
    let tool_call_id = interrupt
        .get("tool_call_id")
        .and_then(Value::as_str)
        .context("Interrupt payload has no tool_call_id")?;
    let empty = Vec::new();
    let questions = interrupt
        .get("questions")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    if questions.is_empty() {
        // Nothing to ask: answering would silently produce `{tool_call_id: {}}`
        // and, in the interactive-resume loop, feed the server an empty
        // answer forever if it keeps replying `interrupted`.
        bail!("Interrupt for tool_call_id={tool_call_id} has no questions to answer");
    }
    let mut answers: HashMap<String, String> = HashMap::new();
    let stdin = std::io::stdin();
    for q in questions {
        let question = q.get("question").and_then(Value::as_str).unwrap_or("?");
        // Already sanitized by `question_choices`.
        let choices: Vec<String> = question_choices(q);
        eprintln!("? {}", strip_control_chars(question));
        for (i, c) in choices.iter().enumerate() {
            eprintln!("  {}. {c}", i + 1);
        }
        eprint!("> ");
        let mut line = String::new();
        let bytes_read = stdin.read_line(&mut line)?;
        if bytes_read == 0 {
            bail!("Unexpected end of input while answering interrupt questions");
        }
        let line = line.trim();
        // A bare number selects an option; anything else is a free-text answer.
        let answer = line
            .parse::<usize>()
            .ok()
            .and_then(|n| choices.get(n.wrapping_sub(1)).map(String::as_str))
            .unwrap_or(line);
        answers.insert(question.to_string(), answer.to_string());
    }
    Ok(json!({ tool_call_id: answers }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_spec_full_form() {
        let v = parse_answer_specs(
            &[
                "call_a:Which period?=1m".to_string(),
                "call_a:Which market?=US".to_string(),
            ],
            None,
        )
        .unwrap();
        assert_eq!(v["call_a"]["Which period?"], "1m");
        assert_eq!(v["call_a"]["Which market?"], "US");
    }

    #[test]
    fn answer_spec_bare_uses_cached_single_question() {
        let interrupt = serde_json::json!({
            "tool_call_id": "call_a",
            "questions": [{"question": "Which period?"}]
        });
        let v = parse_answer_specs(&["1m".to_string()], Some(&interrupt)).unwrap();
        assert_eq!(v["call_a"]["Which period?"], "1m");
    }

    #[test]
    fn answer_spec_bare_without_cache_is_error() {
        let err = parse_answer_specs(&["1m".to_string()], None).unwrap_err();
        assert!(err.to_string().contains("tool_call_id"));
    }

    #[test]
    fn answer_spec_bare_with_multiple_questions_is_error() {
        let interrupt = serde_json::json!({
            "tool_call_id": "call_a",
            "questions": [{"question": "q1"}, {"question": "q2"}]
        });
        assert!(parse_answer_specs(&["1m".to_string()], Some(&interrupt)).is_err());
    }

    #[test]
    fn interrupt_cache_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let interrupt = serde_json::json!({"tool_call_id": "call_a", "questions": []});
        save_interrupt_in(dir.path(), "c1", "m1", &interrupt).unwrap();
        let loaded = load_interrupt_in(dir.path(), "c1", "m1").unwrap();
        assert_eq!(loaded, interrupt);
        clear_interrupt_in(dir.path(), "c1", "m1");
        assert!(load_interrupt_in(dir.path(), "c1", "m1").is_none());
    }

    #[test]
    fn continue_hint_prefills_full_answer_form() {
        let interrupt = serde_json::json!({
            "tool_call_id": "call_a",
            "questions": [{"question": "Which period?", "choices": ["1w", "1m"]}]
        });
        let hint = continue_hint("chatbot", "c1", "m1", &interrupt);
        assert!(hint.contains("longbridge agent continue 'chatbot' 'c1' 'm1'"));
        assert!(hint.contains("call_a:Which period?="));
        assert!(hint.contains("1w") && hint.contains("1m"));
    }

    // ── fix-round-1 regression tests ────────────────────────────────────────

    #[test]
    fn cache_file_sanitizes_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let interrupt = serde_json::json!({"tool_call_id": "call_a", "questions": []});
        // A malicious/buggy chat_uid must not let the write escape `dir`.
        save_interrupt_in(
            dir.path(),
            "../../../../tmp/evil",
            "../../etc/passwd",
            &interrupt,
        )
        .unwrap();
        let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 1, "exactly one file must land inside dir");
        let name = entries[0].as_ref().unwrap().file_name();
        let name = name.to_string_lossy();
        assert!(
            !name.contains(".."),
            "sanitized name still has '..': {name}"
        );
        assert!(!name.contains('/'), "sanitized name still has '/': {name}");
        // And it must still be loadable back through the same sanitization.
        let loaded =
            load_interrupt_in(dir.path(), "../../../../tmp/evil", "../../etc/passwd").unwrap();
        assert_eq!(loaded, interrupt);
    }

    #[test]
    fn cache_file_distinct_dangerous_ids_do_not_collide() {
        // `a/b` and `a.b` both sanitize to `a_b`: only the appended digest of
        // the raw ids keeps them apart.
        let dir = tempfile::tempdir().unwrap();
        let a = serde_json::json!({"tool_call_id": "a"});
        let b = serde_json::json!({"tool_call_id": "b"});
        assert_eq!(
            sanitize_cache_component("a/b"),
            sanitize_cache_component("a.b"),
            "test premise: these ids sanitize identically"
        );
        save_interrupt_in(dir.path(), "a/b", "1", &a).unwrap();
        save_interrupt_in(dir.path(), "a.b", "1", &b).unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        assert_eq!(load_interrupt_in(dir.path(), "a/b", "1").unwrap(), a);
        assert_eq!(load_interrupt_in(dir.path(), "a.b", "1").unwrap(), b);

        // The same split of the raw bytes must not collide either.
        let c = serde_json::json!({"tool_call_id": "c"});
        save_interrupt_in(dir.path(), "x", "1-2", &c).unwrap();
        assert_ne!(
            cache_file(dir.path(), "x", "1-2"),
            cache_file(dir.path(), "x-1", "2")
        );
    }

    #[test]
    fn cache_file_survives_a_known_32bit_digest_collision() {
        // These two ids sanitize to the same `________` component *and*
        // collided under the earlier 32-bit-truncated digest (found by a
        // brute-force probe). With the full 64-bit digest they must not.
        let dir = tempfile::tempdir().unwrap();
        let (a_uid, b_uid) = (">|;(!<:>", "%.&@/.+;");
        assert_eq!(
            sanitize_cache_component(a_uid),
            sanitize_cache_component(b_uid),
            "test premise: the readable part of the filename is identical"
        );
        let a = serde_json::json!({"tool_call_id": "a"});
        let b = serde_json::json!({"tool_call_id": "b"});
        save_interrupt_in(dir.path(), a_uid, "1", &a).unwrap();
        save_interrupt_in(dir.path(), b_uid, "1", &b).unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        assert_eq!(load_interrupt_in(dir.path(), a_uid, "1").unwrap(), a);
        assert_eq!(load_interrupt_in(dir.path(), b_uid, "1").unwrap(), b);
    }

    #[test]
    fn cache_digest_is_16_hex_chars() {
        let name = cache_file(std::path::Path::new("/tmp"), "c1", "m1")
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let digest = name
            .trim_end_matches(".json")
            .rsplit('-')
            .next()
            .unwrap()
            .to_string();
        assert_eq!(digest.len(), 16, "expected a 64-bit digest, got {name}");
        assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn cache_file_is_stable_for_the_same_ids() {
        let dir = std::path::Path::new("/tmp");
        assert_eq!(cache_file(dir, "c1", "m1"), cache_file(dir, "c1", "m1"));
    }

    #[test]
    fn prompt_answers_rejects_empty_questions() {
        let interrupt = serde_json::json!({"tool_call_id": "call_a", "questions": []});
        let err = prompt_answers(&interrupt).unwrap_err();
        assert!(
            err.to_string().contains("no questions"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn backfill_outcome_ids_fills_only_empty_fields() {
        let mut fresh = ChatOutcome::default();
        backfill_outcome_ids(&mut fresh, "c1", "m1");
        assert_eq!(fresh.chat_uid, "c1");
        assert_eq!(fresh.message_id, "m1");

        let mut already_set = ChatOutcome {
            chat_uid: "from_stream".to_string(),
            message_id: "also_from_stream".to_string(),
            ..Default::default()
        };
        backfill_outcome_ids(&mut already_set, "c1", "m1");
        assert_eq!(already_set.chat_uid, "from_stream");
        assert_eq!(already_set.message_id, "also_from_stream");
    }

    #[test]
    fn backfill_outcome_ids_fills_partial() {
        // Only chat_uid missing: message_id from the stream must survive.
        let mut half = ChatOutcome {
            message_id: "from_stream".to_string(),
            ..Default::default()
        };
        backfill_outcome_ids(&mut half, "c1", "m1");
        assert_eq!(half.chat_uid, "c1");
        assert_eq!(half.message_id, "from_stream");
    }

    // ── fix-round-2 regression tests ────────────────────────────────────────

    #[test]
    fn shell_single_quote_escapes_embedded_quotes() {
        assert_eq!(shell_single_quote("abc"), "'abc'");
        assert_eq!(shell_single_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_single_quote(""), "''");
    }

    #[test]
    fn continue_hint_neutralizes_hostile_question() {
        // A hostile question containing `"`, a command substitution, and
        // backticks must not be able to break out of the --answer argument
        // when this hint is copy-pasted into a shell.
        let interrupt = serde_json::json!({
            "tool_call_id": "call_a",
            "questions": [{"question": "Really? \" ; $(rm -rf ~) `whoami`"}]
        });
        let hint = continue_hint("chatbot", "c1", "m1", &interrupt);
        assert!(
            hint.contains("--answer 'call_a:"),
            "answer arg must be single-quoted: {hint}"
        );
        assert!(
            !hint.contains("--answer \""),
            "answer arg must not be double-quoted: {hint}"
        );
    }

    #[test]
    fn continue_hint_escapes_embedded_single_quote_in_question() {
        let interrupt = serde_json::json!({
            "tool_call_id": "call_a",
            "questions": [{"question": "It's now?"}]
        });
        let hint = continue_hint("chatbot", "c1", "m1", &interrupt);
        assert!(
            hint.contains("It'\\''s now?"),
            "embedded single quote must be escaped: {hint}"
        );
    }

    #[test]
    fn continue_hint_strips_control_chars_from_question() {
        let interrupt = serde_json::json!({
            "tool_call_id": "call_a",
            "questions": [{"question": "evil\x1b[31mtext\x1b[0m"}]
        });
        let hint = continue_hint("chatbot", "c1", "m1", &interrupt);
        assert!(!hint.contains('\x1b'), "ESC survived in hint: {hint:?}");
    }

    #[tokio::test]
    async fn unknown_status_json_format_errors_after_printing() {
        let outcome = ChatOutcome {
            status: "unknown".to_string(),
            answer: "partial answer text".to_string(),
            chat_uid: "c1".to_string(),
            message_id: "m1".to_string(),
            ..Default::default()
        };
        let err = handle_outcome("chatbot", outcome, false, false, &OutputFormat::Json, false)
            .await
            .expect_err("dropped stream (status=unknown) must exit non-zero");
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn unknown_status_pretty_format_errors_after_printing() {
        let outcome = ChatOutcome {
            status: "unknown".to_string(),
            answer: "partial answer text".to_string(),
            chat_uid: "c1".to_string(),
            message_id: "m1".to_string(),
            ..Default::default()
        };
        let err = handle_outcome(
            "chatbot",
            outcome,
            false,
            false,
            &OutputFormat::Pretty,
            false,
        )
        .await
        .expect_err("dropped stream (status=unknown) must exit non-zero");
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn succeeded_status_does_not_error() {
        let outcome = ChatOutcome {
            status: "succeeded".to_string(),
            answer: "done".to_string(),
            ..Default::default()
        };
        handle_outcome("chatbot", outcome, false, false, &OutputFormat::Json, false)
            .await
            .expect("succeeded run must not error");
    }

    #[test]
    fn save_interrupt_in_errors_when_dir_path_is_blocked_by_a_file() {
        // Documents the failure mode that `handle_outcome_depth` now treats
        // as best-effort (warn + continue) instead of propagating: a local
        // fs failure (here, a regular file sitting where a directory
        // component is expected) must not be silently swallowed by
        // `save_interrupt_in` itself -- that stays a real `Err`, it's the
        // caller's job to downgrade it to a warning.
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, b"not a directory").unwrap();
        let interrupt = serde_json::json!({"tool_call_id": "call_a"});
        let err = save_interrupt_in(&blocked.join("sub"), "c1", "m1", &interrupt).unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    // ── fix-round-3 regression tests ────────────────────────────────────────

    #[test]
    fn continue_hint_quotes_hostile_ids() {
        let interrupt = serde_json::json!({
            "tool_call_id": "call_a",
            "questions": [{"question": "Which period?"}]
        });
        let hint = continue_hint(
            "chat$(touch /tmp/pwn)bot",
            "c`whoami`1",
            "m;rm -rf ~;1",
            &interrupt,
        );
        let command = hint
            .lines()
            .find(|l| l.starts_with("longbridge agent continue"))
            .expect("command line present");
        assert!(
            command.starts_with(
                "longbridge agent continue 'chat$(touch /tmp/pwn)bot' 'c`whoami`1' 'm;rm -rf ~;1'"
            ),
            "ids must be fully single-quoted: {command}"
        );
    }

    #[test]
    fn follow_up_footer_quotes_hostile_ids() {
        // `print_footer` writes to stdout; assert on the same construction it
        // uses so the quoting is pinned without capturing stdout.
        assert_eq!(shell_arg("c$(touch /tmp/pwn)1"), "'c$(touch /tmp/pwn)1'");
        assert_eq!(shell_arg("m`whoami`"), "'m`whoami`'");
        assert_eq!(shell_arg("id\x1b[31m"), "'id[31m'");
        assert_eq!(shell_arg("it's"), "'it'\\''s'");
    }

    #[test]
    fn answer_spec_question_containing_equals_uses_cache() {
        let interrupt = serde_json::json!({
            "tool_call_id": "call_a",
            "questions": [{"question": "Is P/E=20 acceptable?"}]
        });
        let v = parse_answer_specs(
            &["call_a:Is P/E=20 acceptable?=yes".to_string()],
            Some(&interrupt),
        )
        .unwrap();
        assert_eq!(v["call_a"]["Is P/E=20 acceptable?"], "yes");
    }

    #[test]
    fn answer_spec_longest_cached_question_wins() {
        // "Which period?" is a prefix of "Which period?=or range?"; the
        // longer question must claim the spec.
        let interrupt = serde_json::json!({
            "tool_call_id": "call_a",
            "questions": [
                {"question": "Which period?"},
                {"question": "Which period?=or range?"}
            ]
        });
        let v = parse_answer_specs(
            &["call_a:Which period?=or range?=1m".to_string()],
            Some(&interrupt),
        )
        .unwrap();
        assert_eq!(v["call_a"]["Which period?=or range?"], "1m");
        assert!(v["call_a"].get("Which period?").is_none());
    }

    #[test]
    fn answer_spec_falls_back_to_first_equals_without_cache() {
        let v = parse_answer_specs(&["call_a:Which period?=1m".to_string()], None).unwrap();
        assert_eq!(v["call_a"]["Which period?"], "1m");

        // Cache present but no matching question: same fallback.
        let interrupt = serde_json::json!({
            "tool_call_id": "call_z",
            "questions": [{"question": "Other?"}]
        });
        let v =
            parse_answer_specs(&["call_a:Which period?=1m".to_string()], Some(&interrupt)).unwrap();
        assert_eq!(v["call_a"]["Which period?"], "1m");
    }

    #[test]
    fn classify_outcome_is_an_allowlist() {
        let with = |status: &str, interrupt: Option<Value>| ChatOutcome {
            status: status.to_string(),
            interrupt,
            ..Default::default()
        };
        assert_eq!(classify_outcome(&with("succeeded", None)), Verdict::Success);
        assert_eq!(
            classify_outcome(&with("interrupted", Some(json!({"tool_call_id": "a"})))),
            Verdict::Interrupted
        );
        for status in [
            "interrupted", // payload missing
            "cancelled",
            "failed",
            "stopped",
            "unknown",
            "",
            "some_future_status",
        ] {
            assert!(
                matches!(classify_outcome(&with(status, None)), Verdict::Failure(_)),
                "status {status:?} must not exit 0"
            );
        }
    }

    #[test]
    fn classify_outcome_failure_carries_error_message() {
        let outcome = ChatOutcome {
            status: "failed".to_string(),
            error_message: "tool blew up".to_string(),
            ..Default::default()
        };
        let Verdict::Failure(msg) = classify_outcome(&outcome) else {
            panic!("failed must be a Failure verdict");
        };
        assert!(msg.contains("tool blew up"), "{msg}");
    }

    #[tokio::test]
    async fn novel_status_exits_non_zero_in_both_formats() {
        for format in [OutputFormat::Json, OutputFormat::Pretty] {
            let outcome = ChatOutcome {
                status: "cancelled".to_string(),
                answer: "partial".to_string(),
                ..Default::default()
            };
            handle_outcome("chatbot", outcome, false, false, &format, false)
                .await
                .expect_err("a non-succeeded status must exit non-zero");
        }
    }

    #[tokio::test]
    async fn interrupted_without_payload_exits_non_zero() {
        let outcome = ChatOutcome {
            status: "interrupted".to_string(),
            ..Default::default()
        };
        let err = handle_outcome("chatbot", outcome, false, false, &OutputFormat::Json, false)
            .await
            .expect_err("interrupted with no payload must exit non-zero");
        assert!(err.to_string().contains("interrupt payload"), "{err}");
    }

    // ── fix-round-4 regression tests ────────────────────────────────────────

    #[test]
    fn answers_json_accepts_the_raw_payload() {
        let v = parse_answers_json(r#"{"call_a":{"Is P/E=20 acceptable?":"yes"}}"#).unwrap();
        assert_eq!(v["call_a"]["Is P/E=20 acceptable?"], "yes");
        // The request body wraps it verbatim.
        let body = json!({ "answers_by_tool_call": v });
        assert_eq!(
            body["answers_by_tool_call"]["call_a"]["Is P/E=20 acceptable?"],
            "yes"
        );
    }

    #[test]
    fn answers_json_rejects_wrong_shapes() {
        for bad in [
            "not json",
            "[]",
            "\"str\"",
            "{}",
            r#"{"call_a":"1m"}"#,
            r#"{"call_a":{}}"#,
            r#"{"call_a":{"q":1}}"#,
            r#"{"call_a":{"q":null}}"#,
        ] {
            assert!(parse_answers_json(bad).is_err(), "must be rejected: {bad}");
        }
    }

    #[test]
    fn classify_outcome_sanitizes_server_text_in_the_bail() {
        let outcome = ChatOutcome {
            status: "fai\x1b[31mled".to_string(),
            error_message: "boom\x1b]0;pwn\x07".to_string(),
            ..Default::default()
        };
        let Verdict::Failure(msg) = classify_outcome(&outcome) else {
            panic!("must be a failure");
        };
        assert!(!msg.contains('\x1b'), "ESC survived in bail: {msg:?}");
        assert!(!msg.contains('\x07'), "BEL survived in bail: {msg:?}");
        assert!(msg.contains("boom"));
    }

    #[test]
    fn interactive_is_rejected_for_json_output() {
        let err = ensure_interactive_supported(true, &OutputFormat::Json).unwrap_err();
        assert!(err.to_string().contains("--answer"), "{err}");
        ensure_interactive_supported(true, &OutputFormat::Pretty).unwrap();
        ensure_interactive_supported(false, &OutputFormat::Json).unwrap();
    }

    #[test]
    fn question_choices_reads_the_real_options_shape() {
        // What the server actually sends.
        let q = json!({
            "question": "Which symbol?",
            "multi_select": false,
            "options": [{"description": "TQQQ"}, {"description": "SOXL"}]
        });
        assert_eq!(question_choices(&q), vec!["TQQQ", "SOXL"]);

        // Reading `choices` instead produced an empty list, which is what
        // blanked the interactive prompt; the legacy shape still works.
        assert_eq!(
            question_choices(&json!({"choices": ["a", "b"]})),
            vec!["a", "b"]
        );
        // Options of bare strings, and no options at all.
        assert_eq!(question_choices(&json!({"options": ["x"]})), vec!["x"]);
        assert!(question_choices(&json!({"question": "free text"})).is_empty());
    }

    #[test]
    fn question_choices_sanitizes_option_text() {
        let q = json!({"options": [{"description": "TQQQ\x1b[31m\x07"}]});
        let got = question_choices(&q);
        assert!(!got[0].contains('\x1b'), "ESC survived: {got:?}");
        assert!(!got[0].contains('\x07'), "BEL survived: {got:?}");
    }

    #[test]
    fn interactive_guard_reads_the_subcommand() {
        use crate::cli::AgentCmd;
        let chat = AgentCmd::Chat {
            agent_uid: "a".into(),
            args: vec!["q".into()],
            chat_uid: None,
            parent_message_id: None,
            stream: false,
            interactive: true,
        };
        assert!(ensure_interactive_supported_for(&chat, &OutputFormat::Json).is_err());
        assert!(ensure_interactive_supported_for(&chat, &OutputFormat::Pretty).is_ok());

        // `continue` shares the arm with `chat`; assert it rather than
        // assuming the shared match covers it.
        let cont = AgentCmd::Continue {
            agent_uid: "a".into(),
            ids: vec!["ct".into(), "1".into()],
            chat_uid: None,
            message_id: None,
            answer: vec![],
            answers_json: None,
            interactive: true,
        };
        assert!(ensure_interactive_supported_for(&cont, &OutputFormat::Json).is_err());
        assert!(ensure_interactive_supported_for(&cont, &OutputFormat::Pretty).is_ok());

        // A non-interactive Continue is fine in either format.
        let cont_quiet = AgentCmd::Continue {
            agent_uid: "a".into(),
            ids: vec!["ct".into(), "1".into()],
            chat_uid: None,
            message_id: None,
            answer: vec!["x".into()],
            answers_json: None,
            interactive: false,
        };
        assert!(ensure_interactive_supported_for(&cont_quiet, &OutputFormat::Json).is_ok());

        // `list` has no --interactive, so it is never rejected.
        let list = AgentCmd::List {
            workspace: None,
            name: None,
            published: false,
            all: false,
            page: 1,
            count: 20,
        };
        assert!(ensure_interactive_supported_for(&list, &OutputFormat::Json).is_ok());
    }

    /// `continue` enforced the pretty-only rule while `chat` did not, so
    /// `chat --interactive --format json` ran and returned JSON. The guard is
    /// the first statement of `cmd_chat`, so this rejects before any request:
    /// the bogus agent uid below is never dialled.
    #[tokio::test]
    async fn chat_interactive_is_rejected_for_json_before_any_request() {
        let target = crate::cli::agent::ChatTarget {
            agent_uid: "unreachable-agent".to_string(),
            query: "hi".to_string(),
            chat_uid: None,
            parent_message_id: None,
        };
        let err = cmd_chat(target, false, true, &OutputFormat::Json, false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("--answer"), "{err}");
    }

    // ── fix-round-5 regression tests ────────────────────────────────────────

    /// Blocker 3 regression: `--answers-json` is written by an AI harness,
    /// so a malformed payload full of ANSI/OSC sequences must not reach stderr
    /// verbatim. Covers all three interpolation sites: the raw input, the
    /// decoded `tool_call_id` key, and the decoded question key.
    #[test]
    fn answers_json_errors_are_free_of_control_chars() {
        let hostile = [
            // Invalid JSON — the raw input is echoed back.
            "\x1b]0;pwned\x07{not json\x1b[31m",
            // Valid JSON, wrong value shape — the tool_call_id key is echoed.
            "{\"ca\x1b[31mll\x07\":\"1m\"}",
            // Valid JSON, empty answers — the tool_call_id key is echoed.
            "{\"ca\x1b]0;x\x07ll\":{}}",
            // Valid JSON, non-string answer — both keys are echoed.
            "{\"ca\x1bll\":{\"q\x1b[2Juestion\":1}}",
        ];
        for raw in hostile {
            let Err(err) = parse_answers_json(raw) else {
                panic!("must be rejected: {raw:?}");
            };
            // `anyhow` context chains: check every link, not just the head.
            let rendered = format!("{err:#}");
            for bad in ['\x1b', '\x07'] {
                assert!(
                    !rendered.contains(bad),
                    "control char {bad:?} survived into the error for {raw:?}: {rendered:?}"
                );
            }
        }
    }

    /// A long hostile payload is clipped rather than dumped wholesale.
    #[test]
    fn answers_json_error_truncates_long_payloads() {
        let raw = format!("{{{}", "A".repeat(5_000));
        let err = parse_answers_json(&raw).expect_err("must be rejected");
        let rendered = format!("{err:#}");
        assert!(rendered.len() < 500, "payload was not clipped: {rendered}");
        assert!(rendered.contains('…'), "no truncation marker: {rendered}");
    }

    /// Non-control text still survives, so the message stays diagnostic.
    #[test]
    fn answers_json_error_keeps_readable_text() {
        let err =
            parse_answers_json(r#"{"call_a":{"Which period?":1}}"#).expect_err("must be rejected");
        let rendered = format!("{err:#}");
        assert!(rendered.contains("call_a"), "{rendered}");
        assert!(rendered.contains("Which period?"), "{rendered}");
    }

    /// Blocker 4 regression, driven through clap exactly as the binary is:
    /// `--answers-json --interactive --format json` must be rejected by the
    /// pretty-only interactive guard *before* the `--answers-json` branch
    /// returns. The payload is valid JSON, so only the guard can reject it.
    ///
    /// `resolve_continue_answers` is everything `cmd_continue` does before it
    /// builds a request, so this covers the ordering without any I/O.
    #[test]
    fn answers_json_does_not_bypass_the_interactive_guard() {
        // clap's debug-time build of this broad command tree is recursive and
        // exceeds the test harness's 2 MiB worker stack; run it on a bigger one.
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use crate::cli::{AgentCmd, Cli, Commands};
                use clap::Parser;

                let cli = Cli::try_parse_from([
                    "longbridge",
                    "--format",
                    "json",
                    "agent",
                    "continue",
                    "chatbot",
                    "ct_1",
                    "13025051",
                    "--interactive",
                    "--answers-json",
                    r#"{"call_a":{"Which period?":"1m"}}"#,
                ])
                .expect("clap must accept the flags; the guard is what rejects them");

                let Some(Commands::Agent {
                    cmd:
                        Some(AgentCmd::Continue {
                            ids,
                            answers_json,
                            interactive,
                            ..
                        }),
                    ..
                }) = cli.command
                else {
                    panic!("expected Agent Continue");
                };
                assert!(interactive);
                assert!(matches!(cli.format, OutputFormat::Json));

                let err = resolve_continue_answers(
                    &ids[0],
                    &ids[1],
                    &[],
                    answers_json.as_deref(),
                    interactive,
                    &cli.format,
                )
                .expect_err("--answers-json --interactive --format json must be rejected");
                assert!(
                    err.to_string().contains("--interactive requires"),
                    "expected the pretty-only guard, got: {err}"
                );
            })
            .expect("spawn interactive-guard thread")
            .join()
            .expect("interactive-guard thread");
    }

    /// The guard must not over-reach: the same payload without `--interactive`
    /// (and with `--interactive` under pretty output) still resolves.
    #[test]
    fn answers_json_still_works_when_the_guard_does_not_apply() {
        let raw = r#"{"call_a":{"Which period?":"1m"}}"#;
        for (interactive, format) in [
            (false, OutputFormat::Json),
            (true, OutputFormat::Pretty),
            (false, OutputFormat::Pretty),
        ] {
            let v = resolve_continue_answers("ct_1", "1", &[], Some(raw), interactive, &format)
                .unwrap_or_else(|e| panic!("must be accepted ({interactive}, {format:?}): {e}"));
            assert_eq!(v["call_a"]["Which period?"], "1m");
        }
    }
}
