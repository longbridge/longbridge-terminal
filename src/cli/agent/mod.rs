//! AI agent commands (A2A): discovery, chat, interrupt continuation.

use std::collections::BTreeMap;

use anyhow::{bail, Result};
use rust_i18n::t;

use client::{AgentApi, AgentInfo, LbAgentApi};

pub mod chat;
pub mod client;
pub mod events;
pub mod render;
pub mod skills;

/// Agent modes `agent chat` can drive. Anything else is hidden from
/// `agent list` unless `--all` is passed.
///
/// An allowlist keeps undriveable modes out of the listing even when the
/// server invents new ones — the whole point of the filter. Its danger is the
/// opposite case: `agentic_chat` shipped after `chat` and, being unlisted,
/// silently emptied the listing for accounts holding only new-style agents.
/// So callers must report what was hidden (see `collect_agents`); an agent
/// may be filtered out, but never without saying so.
pub(crate) const CHAT_MODES: &[&str] = &["chat", "agentic_chat"];

/// Whether `agent chat` can hold a conversation with this agent.
pub(crate) fn is_chat_capable(mode: &str) -> bool {
    CHAT_MODES.contains(&mode)
}

/// Resolved chat invocation after merging positional and flag forms.
#[derive(Debug)]
pub struct ChatTarget {
    pub agent_uid: String,
    pub query: String,
    pub chat_uid: Option<String>,
    pub parent_message_id: Option<String>,
}

pub(crate) fn resolve_chat_args(
    agent_uid: String,
    positionals: Vec<String>,
    chat_uid: Option<String>,
    parent_message_id: Option<String>,
) -> Result<ChatTarget> {
    match positionals.len() {
        1 => {
            // A follow-up needs both IDs. Accepting one alone would silently
            // start a *new* conversation while the user believes they are
            // continuing the old one.
            if chat_uid.is_some() != parent_message_id.is_some() {
                bail!(
                    "--chat-uid and --parent-message-id must be given together (or neither).\n\
                     First round:  longbridge agent chat <AGENT_UID> \"<query>\"\n\
                     Follow-up:    longbridge agent chat <AGENT_UID> \"<query>\" \
                     --chat-uid <CHAT_UID> --parent-message-id <MESSAGE_ID>"
                );
            }
            Ok(ChatTarget {
                agent_uid,
                query: positionals.into_iter().next().unwrap(),
                chat_uid,
                parent_message_id,
            })
        }
        3 => {
            if chat_uid.is_some() || parent_message_id.is_some() {
                bail!(
                    "IDs were given both positionally and via --chat-uid/--parent-message-id; use one form"
                );
            }
            let mut it = positionals.into_iter();
            let chat_uid = it.next().unwrap();
            let parent_message_id = it.next().unwrap();
            let query = it.next().unwrap();
            Ok(ChatTarget {
                agent_uid,
                query,
                chat_uid: Some(chat_uid),
                parent_message_id: Some(parent_message_id),
            })
        }
        n => bail!(
            "Expected 1 or 3 positional arguments after AGENT_UID (got {n}).\n\
             First round:  longbridge agent chat <AGENT_UID> \"<query>\"\n\
             Follow-up:    longbridge agent chat <AGENT_UID> <CHAT_UID> <PARENT_MESSAGE_ID> \"<query>\""
        ),
    }
}

pub(crate) fn resolve_continue_ids(
    ids: Vec<String>,
    chat_uid: Option<String>,
    message_id: Option<String>,
) -> Result<(String, String)> {
    match (ids.len(), chat_uid, message_id) {
        (2, None, None) => {
            let mut it = ids.into_iter();
            Ok((it.next().unwrap(), it.next().unwrap()))
        }
        (0, Some(c), Some(m)) => Ok((c, m)),
        (2, _, _) => bail!("IDs were given both positionally and via flags; use one form"),
        _ => bail!(
            "chat_uid and message_id are required.\n\
             Positional: longbridge agent continue <AGENT_UID> <CHAT_UID> <MESSAGE_ID> --answer …\n\
             Flags:      longbridge agent continue <AGENT_UID> --chat-uid=… --message-id=… --answer …"
        ),
    }
}

/// Label shown in the WORKSPACE column for agents that belong to no
/// workspace of yours.
pub(crate) const PUBLIC_WORKSPACE_LABEL: &str = "Public: Longbridge";

/// Agents any account can talk to but that no listing endpoint returns.
///
/// `GET /v1/ai/workspaces/:id/agents` is the only way to enumerate agents and
/// it is scoped to a workspace you own, while `conversations` merely requires
/// the agent to be published. So the set you can chat with is strictly larger
/// than the set you can discover — an AI harness reading `agent list` would
/// never learn these exist. Seed them here until the server exposes a
/// public-agent listing, then delete this table.
/// `(uid, name, description i18n key)`. The description is localized at
/// build time via [`t!`] rather than hardcoded, per the repo's i18n rule; if
/// this table ever holds more than one agent, give each its own key.
const PUBLIC_AGENTS: &[(&str, &str, &str)] =
    &[("chatbot", "LongbridgeAI", "Agent.PublicChatbotDescription")];

/// Build the synthetic entries for [`PUBLIC_AGENTS`].
fn public_agents() -> Vec<AgentInfo> {
    PUBLIC_AGENTS
        .iter()
        .map(|(uid, name, description_key)| AgentInfo {
            uid: (*uid).to_string(),
            name: (*name).to_string(),
            description: t!(*description_key).to_string(),
            mode: CHAT_MODES[0].to_string(),
            is_published: true,
            workspace_id: PUBLIC_WORKSPACE_LABEL.to_string(),
            workspace_name: PUBLIC_WORKSPACE_LABEL.to_string(),
        })
        .collect()
}

/// Agents that survived filtering, plus what the mode filter removed.
///
/// The counts exist so a short listing is never silent: an allowlist cannot
/// know about a chat-capable mode the server adds later, so the user has to
/// be told something was withheld and that `--all` reveals it.
pub(crate) struct AgentListing {
    pub agents: Vec<AgentInfo>,
    /// Hidden mode -> how many agents had it. Empty when nothing was hidden.
    pub hidden_modes: BTreeMap<String, usize>,
}

impl AgentListing {
    /// Only the agents the server actually returned, dropping the seeded
    /// public ones. Lets a test about traversal, paging or filtering assert
    /// on server data without restating the public table.
    #[cfg(test)]
    pub fn agents_from_server(self) -> Vec<AgentInfo> {
        self.agents
            .into_iter()
            .filter(|a| a.workspace_id != PUBLIC_WORKSPACE_LABEL)
            .collect()
    }
}

/// Gather agents: one workspace (with paging controls) or all workspaces
/// (sequential traversal, internal paging with limit 100).
pub(crate) async fn collect_agents(
    api: &dyn AgentApi,
    workspace: Option<String>,
    name: Option<String>,
    published: bool,
    all_modes: bool,
    page: u32,
    count: u32,
) -> Result<AgentListing> {
    let mut all = Vec::new();
    if let Some(ws) = workspace {
        let page_data = api.list_agents(ws.clone(), page, count, name).await?;
        all.extend(page_data.agents.into_iter().map(|mut a| {
            a.workspace_id.clone_from(&ws);
            a
        }));
    } else {
        // Fetch each workspace's agents concurrently rather than serially:
        // discovery is O(latency) instead of O(workspaces × latency). The
        // shared rate limiter inside `list_agents` still bounds throughput to
        // 10 req/s, and `join_all` preserves workspace order. Paging within a
        // single workspace stays sequential — `total` is only known after the
        // first page.
        let workspaces = api.list_workspaces().await?;
        let per_workspace = futures::future::try_join_all(workspaces.iter().map(|ws| {
            let name = name.clone();
            async move {
                let mut ws_agents = Vec::new();
                let mut fetched: u32 = 0;
                let mut p: u32 = 1;
                loop {
                    let page_data = api.list_agents(ws.id.clone(), p, 100, name.clone()).await?;
                    let got = page_data.agents.len() as u32;
                    ws_agents.extend(page_data.agents.into_iter().map(|mut a| {
                        a.workspace_id.clone_from(&ws.id);
                        a.workspace_name.clone_from(&ws.name);
                        a
                    }));
                    fetched += got;
                    if got == 0 || fetched >= page_data.total {
                        break;
                    }
                    p += 1;
                }
                Ok::<_, anyhow::Error>(ws_agents)
            }
        }))
        .await?;
        all.extend(per_workspace.into_iter().flatten());
        // Only when listing across workspaces: `--workspace` asks about one
        // specific workspace, and these belong to none of them.
        for extra in public_agents() {
            // Someone who owns the workspace a public agent lives in already
            // got the real record; do not shadow it with the stub.
            if all.iter().any(|a| a.uid == extra.uid) {
                continue;
            }
            // `--name` is a server-side filter, so apply it here by hand.
            if let Some(needle) = &name {
                let hit = extra.name.to_lowercase().contains(&needle.to_lowercase());
                if !hit {
                    continue;
                }
            }
            all.push(extra);
        }
    }
    if published {
        all.retain(|a| a.is_published);
    }
    // Workflow agents cannot hold a conversation: the server rejects
    // `agent chat` against one with an empty `error_message`, which surfaces
    // as a bare "status=failed" the user cannot act on. Hide them until a
    // dedicated `longbridge workflow` command exists. `--all` still reveals
    // them, so nothing a user created ever becomes truly invisible.
    let mut hidden_modes: BTreeMap<String, usize> = BTreeMap::new();
    if !all_modes {
        all.retain(|a| {
            let keep = is_chat_capable(&a.mode);
            if !keep {
                *hidden_modes.entry(a.mode.clone()).or_default() += 1;
            }
            keep
        });
    }
    Ok(AgentListing {
        agents: all,
        hidden_modes,
    })
}

/// Render a server-supplied mode name for a one-line diagnostic.
///
/// The mode reaches us from the API, so it gets the same treatment the table
/// output already gives it: strip control characters, then flatten and cap it
/// so a hostile value cannot smuggle newlines into the note or flood stderr.
pub(crate) fn render_mode_label(mode: &str) -> String {
    const MAX: usize = 40;
    let flat =
        crate::cli::agent::render::strip_control_chars(mode).replace(['\n', '\r', '\t'], " ");
    let flat = flat.trim();
    if flat.is_empty() {
        return "<empty>".to_string();
    }
    match flat.char_indices().nth(MAX) {
        Some((cut, _)) => format!("{}…", &flat[..cut]),
        None => flat.to_string(),
    }
}

/// Tell the user what the mode filter withheld, on stderr so it never
/// contaminates `--format json` on stdout.
fn warn_about_hidden(hidden: &BTreeMap<String, usize>) {
    if hidden.is_empty() {
        return;
    }
    let detail = hidden
        .iter()
        .map(|(mode, n)| format!("{n} {}", render_mode_label(mode)))
        .collect::<Vec<_>>()
        .join(", ");
    let total: usize = hidden.values().sum();
    eprintln!(
        "note: {total} agent(s) hidden because `agent chat` cannot drive their mode ({detail}); \
         pass --all to list them"
    );
}

pub async fn cmd_agent(
    cmd: Option<crate::cli::AgentCmd>,
    skill: bool,
    format: &crate::cli::OutputFormat,
    verbose: bool,
) -> Result<()> {
    use crate::cli::AgentCmd;
    if skill {
        skills::print_skills_doc();
        return Ok(());
    }
    match cmd {
        None => {
            // Bare `longbridge agent`: show what the group offers rather than
            // guessing a subcommand. See `exit_with_subcommand_help`.
            crate::cli::exit_with_subcommand_help("agent")
        }
        Some(AgentCmd::List {
            workspace,
            name,
            published,
            all,
            page,
            count,
        }) => {
            cmd_list(
                workspace, name, published, all, page, count, format, verbose,
            )
            .await
        }
        Some(AgentCmd::Chat {
            agent_uid,
            args,
            chat_uid,
            parent_message_id,
            stream,
            interactive,
        }) => {
            let target = resolve_chat_args(agent_uid, args, chat_uid, parent_message_id)?;
            chat::cmd_chat(target, stream, interactive, format, verbose).await
        }
        Some(AgentCmd::Continue {
            agent_uid,
            ids,
            chat_uid,
            message_id,
            answer,
            answers_json,
            interactive,
        }) => {
            let (chat_uid, message_id) = resolve_continue_ids(ids, chat_uid, message_id)?;
            chat::cmd_continue(
                agent_uid,
                chat_uid,
                message_id,
                answer,
                answers_json,
                interactive,
                format,
                verbose,
            )
            .await
        }
    }
}

async fn cmd_list(
    workspace: Option<String>,
    name: Option<String>,
    published: bool,
    all_modes: bool,
    page: u32,
    count: u32,
    format: &crate::cli::OutputFormat,
    verbose: bool,
) -> Result<()> {
    use crate::cli::output::{print_json_value, print_table};
    use crate::cli::OutputFormat;
    let api = LbAgentApi { verbose };
    let listing = collect_agents(&api, workspace, name, published, all_modes, page, count).await?;
    let agents = listing.agents;
    warn_about_hidden(&listing.hidden_modes);
    match format {
        OutputFormat::Json => {
            print_json_value(&serde_json::json!({ "agents": agents }), format);
        }
        OutputFormat::Pretty => {
            let rows = agent_rows(&agents);
            print_table(
                &[
                    "UID",
                    "NAME",
                    "MODE",
                    "PUBLISHED",
                    "WORKSPACE",
                    "DESCRIPTION",
                ],
                rows,
                format,
            );
        }
    }
    Ok(())
}

/// Build the pretty-table rows for `agent list`.
///
/// Every column is server-supplied text that lands on the terminal verbatim,
/// so each cell is stripped of control characters (an agent named with an
/// embedded OSC/SGR sequence could otherwise repaint the table or the title
/// bar). JSON output is untouched: `serde_json` escapes control characters.
fn agent_rows(agents: &[AgentInfo]) -> Vec<Vec<String>> {
    use render::strip_control_chars;
    agents
        .iter()
        .map(|a| {
            vec![
                strip_control_chars(&a.uid),
                strip_control_chars(&a.name),
                strip_control_chars(&a.mode),
                if a.is_published {
                    "yes".into()
                } else {
                    "no".into()
                },
                strip_control_chars(&a.workspace_id),
                strip_control_chars(&crate::cli::news::truncate_display(&a.description, 40)),
            ]
        })
        .collect()
}

pub(crate) fn schema_for_path(path: &[String]) -> Option<crate::cli::schema::ResponseSchema> {
    use crate::cli::schema::{self, field, RootKind};

    let schema = match path.get(1).map(String::as_str) {
        Some("chat" | "continue") => schema::schema(
            "Agent conversation result",
            RootKind::Object,
            vec![
                field("chat_uid", "string", "Conversation ID for follow-ups"),
                field("message_id", "string", "Message ID of this round"),
                field(
                    "status",
                    "string",
                    "succeeded | interrupted | failed | stopped | unknown",
                ),
                field("answer", "string", "Answer body as raw markdown"),
                field(
                    "widgets",
                    "object[]",
                    "Embedded widgets: {kind: \"vis-chart\", spec} | {kind: \"x-widget\", src}",
                ),
                field(
                    "references",
                    "object[]",
                    "Cited sources: {type, id, index, content{…}}",
                ),
                field(
                    "further_questions",
                    "string[]",
                    "Suggested follow-up questions",
                ),
                field(
                    "elapsed_time",
                    "number | null",
                    "Run duration in seconds; null when the run did not finish",
                ),
                field(
                    "interrupt",
                    "object | null",
                    "Present when status=interrupted: {tool_call_id, questions[]}. \
                     Each question is an OBJECT, not a string: \
                     {question, multi_select, options[{description}]}. \
                     Answer keys must be the inner `question` value",
                ),
                field(
                    "error_message",
                    "string",
                    "Failure detail; empty on success",
                ),
            ],
        ),
        // Bare `longbridge agent` runs `agent list`, so `agent --schema`
        // must describe the list response instead of falling through to help.
        None | Some("list") => schema::object("AI agents across workspaces", &["agents"]),
        _ => return None,
    };
    Some(schema)
}

#[cfg(test)]
mod tests {
    use super::client::{AgentPage, MockAgentApi, WorkspaceInfo};
    use super::*;

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn chat_one_positional_is_first_round() {
        let t = resolve_chat_args(s("chatbot"), vec![s("hello")], None, None).unwrap();
        assert_eq!(t.query, "hello");
        assert!(t.chat_uid.is_none() && t.parent_message_id.is_none());
    }

    #[test]
    fn chat_three_positionals_are_followup() {
        let t = resolve_chat_args(
            s("chatbot"),
            vec![s("ct_1"), s("99"), s("more")],
            None,
            None,
        )
        .unwrap();
        assert_eq!(t.chat_uid.as_deref(), Some("ct_1"));
        assert_eq!(t.parent_message_id.as_deref(), Some("99"));
        assert_eq!(t.query, "more");
    }

    #[test]
    fn chat_two_positionals_is_error() {
        let err =
            resolve_chat_args(s("chatbot"), vec![s("ct_1"), s("more")], None, None).unwrap_err();
        assert!(err.to_string().contains("1 or 3"));
    }

    #[test]
    fn chat_flag_and_positional_ids_conflict() {
        let err = resolve_chat_args(
            s("chatbot"),
            vec![s("ct_1"), s("99"), s("more")],
            Some(s("ct_2")),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("both"));
    }

    #[test]
    fn chat_one_flag_alone_is_error() {
        for (chat_uid, parent) in [(Some(s("ct_1")), None), (None, Some(s("99")))] {
            let err = resolve_chat_args(s("chatbot"), vec![s("more")], chat_uid, parent)
                .expect_err("a lone follow-up ID must be rejected");
            assert!(
                err.to_string().contains("together"),
                "unexpected error: {err}"
            );
        }
    }

    #[test]
    fn agent_rows_strip_control_chars() {
        let mut a = agent("chat\x1b[31mbot", true);
        a.name = "Long\x1b]0;pwn\x07bridge".to_string();
        a.mode = "chat\x07".to_string();
        a.description = "desc\x1b[0m".to_string();
        a.workspace_id = "33\x1b[1m".to_string();
        let rows = agent_rows(&[a]);
        let joined = rows[0].join("|");
        assert!(!joined.contains('\x1b'), "ESC survived: {joined:?}");
        assert!(!joined.contains('\x07'), "BEL survived: {joined:?}");
        assert!(joined.contains("bot") && joined.contains("bridge"));
    }

    #[test]
    fn bare_agent_path_maps_to_list_schema() {
        let bare = schema_for_path(&[s("agent")]).expect("bare `agent` must have a schema");
        let list = schema_for_path(&[s("agent"), s("list")]).expect("list schema");
        assert_eq!(bare.summary, list.summary);
        assert!(bare.fields.iter().any(|f| f.name == "agents"));
    }

    #[test]
    fn chat_schema_declares_typed_fields() {
        let schema = schema_for_path(&[s("agent"), s("chat")]).expect("chat schema");
        let ty = |name: &str| {
            schema
                .fields
                .iter()
                .find(|f| f.name == name)
                .map_or_else(|| panic!("missing field {name}"), |f| f.ty.clone())
        };
        assert_eq!(ty("widgets"), "object[]");
        assert_eq!(ty("references"), "object[]");
        assert_eq!(ty("further_questions"), "string[]");
        assert_eq!(ty("elapsed_time"), "number | null");
        assert_eq!(ty("error_message"), "string");
    }

    #[test]
    fn chat_flags_apply_to_single_positional() {
        let t = resolve_chat_args(
            s("chatbot"),
            vec![s("more")],
            Some(s("ct_1")),
            Some(s("99")),
        )
        .unwrap();
        assert_eq!(t.chat_uid.as_deref(), Some("ct_1"));
        assert_eq!(t.parent_message_id.as_deref(), Some("99"));
    }

    #[test]
    fn continue_ids_from_positional() {
        assert_eq!(
            resolve_continue_ids(vec![s("ct_1"), s("99")], None, None).unwrap(),
            (s("ct_1"), s("99"))
        );
    }

    #[test]
    fn continue_ids_from_flags() {
        assert_eq!(
            resolve_continue_ids(Vec::new(), Some(s("ct_1")), Some(s("99"))).unwrap(),
            (s("ct_1"), s("99"))
        );
    }

    #[test]
    fn continue_ids_missing_is_error() {
        assert!(resolve_continue_ids(Vec::new(), None, None).is_err());
        assert!(resolve_continue_ids(vec![s("ct_1")], None, None).is_err());
    }

    #[test]
    fn continue_ids_double_source_is_error() {
        let err =
            resolve_continue_ids(vec![s("ct_1"), s("99")], Some(s("ct_2")), None).unwrap_err();
        assert!(err.to_string().contains("both"));
    }

    fn agent(uid: &str, published: bool) -> super::client::AgentInfo {
        super::client::AgentInfo {
            uid: uid.to_string(),
            name: format!("name-{uid}"),
            description: String::new(),
            mode: "chat".to_string(),
            is_published: published,
            workspace_id: String::new(),
            workspace_name: String::new(),
        }
    }

    #[tokio::test]
    async fn collect_agents_traverses_all_workspaces() {
        let mut api = MockAgentApi::new();
        api.expect_list_workspaces().times(1).returning(|| {
            Ok(vec![
                WorkspaceInfo {
                    id: "1".into(),
                    name: "w1".into(),
                    created_at: 0,
                    updated_at: 0,
                },
                WorkspaceInfo {
                    id: "2".into(),
                    name: "w2".into(),
                    created_at: 0,
                    updated_at: 0,
                },
            ])
        });
        api.expect_list_agents()
            .times(2)
            .returning(|ws, _page, _limit, _name| {
                let uid = format!("a{ws}");
                Ok(AgentPage {
                    agents: vec![agent(&uid, true)],
                    total: 1,
                })
            });
        let agents = collect_agents(&api, None, None, false, false, 1, 20)
            .await
            .unwrap()
            .agents_from_server();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].workspace_id, "1");
        assert_eq!(agents[0].workspace_name, "w1");
        assert_eq!(agents[1].workspace_id, "2");
    }

    #[tokio::test]
    async fn collect_agents_paginates_within_a_workspace() {
        let mut api = MockAgentApi::new();
        api.expect_list_workspaces().times(1).returning(|| {
            Ok(vec![WorkspaceInfo {
                id: "1".into(),
                name: "w1".into(),
                created_at: 0,
                updated_at: 0,
            }])
        });
        // total=150 with limit=100 -> two pages
        api.expect_list_agents()
            .times(2)
            .returning(|_, page, _, _| {
                let n = if page == 1 { 100 } else { 50 };
                Ok(AgentPage {
                    agents: (0..n)
                        .map(|i| agent(&format!("p{page}-{i}"), true))
                        .collect(),
                    total: 150,
                })
            });
        let agents = collect_agents(&api, None, None, false, false, 1, 20)
            .await
            .unwrap()
            .agents_from_server();
        assert_eq!(agents.len(), 150);
    }

    #[tokio::test]
    async fn collect_agents_single_workspace_uses_page_and_count() {
        let mut api = MockAgentApi::new();
        api.expect_list_workspaces().times(0);
        api.expect_list_agents()
            .withf(|ws, page, limit, _| ws == "33" && *page == 2 && *limit == 5)
            .times(1)
            .returning(|_, _, _, _| {
                Ok(AgentPage {
                    agents: vec![agent("x", true)],
                    total: 100,
                })
            });
        let agents = collect_agents(&api, Some("33".into()), None, false, false, 2, 5)
            .await
            .unwrap()
            .agents_from_server();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].workspace_id, "33");
    }

    #[tokio::test]
    async fn collect_agents_published_filter() {
        let mut api = MockAgentApi::new();
        api.expect_list_workspaces().times(1).returning(|| {
            Ok(vec![WorkspaceInfo {
                id: "1".into(),
                name: "w1".into(),
                created_at: 0,
                updated_at: 0,
            }])
        });
        api.expect_list_agents().times(1).returning(|_, _, _, _| {
            Ok(AgentPage {
                agents: vec![agent("pub", true), agent("draft", false)],
                total: 2,
            })
        });
        let agents = collect_agents(&api, None, None, true, false, 1, 20)
            .await
            .unwrap()
            .agents_from_server();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].uid, "pub");
    }

    fn agent_in_mode(uid: &str, mode: &str) -> super::client::AgentInfo {
        super::client::AgentInfo {
            mode: mode.to_string(),
            ..agent(uid, true)
        }
    }

    /// One workspace serving one chat agent and one workflow agent.
    fn mixed_mode_api() -> MockAgentApi {
        let mut api = MockAgentApi::new();
        api.expect_list_workspaces().times(1).returning(|| {
            Ok(vec![WorkspaceInfo {
                id: "1".into(),
                name: "w1".into(),
                created_at: 0,
                updated_at: 0,
            }])
        });
        api.expect_list_agents().times(1).returning(|_, _, _, _| {
            Ok(AgentPage {
                agents: vec![
                    agent_in_mode("talker", "chat"),
                    agent_in_mode("runner", "workflow"),
                ],
                total: 2,
            })
        });
        api
    }

    #[tokio::test]
    async fn collect_agents_hides_workflow_mode_by_default() {
        let agents = collect_agents(&mixed_mode_api(), None, None, false, false, 1, 20)
            .await
            .unwrap()
            .agents_from_server();
        assert_eq!(agents.len(), 1, "workflow agents must not be listed");
        assert_eq!(agents[0].uid, "talker");
    }

    /// `agentic_chat` shipped after `chat` and, being absent from the
    /// allowlist, emptied the listing for an account whose only agent was of
    /// that kind — with nothing on screen to explain it. It must be listed
    /// now, and anything still withheld must be counted so the caller can
    /// tell the user rather than showing a silently short list.
    #[tokio::test]
    async fn collect_agents_lists_agentic_chat_and_counts_what_it_hides() {
        let mut api = MockAgentApi::new();
        api.expect_list_workspaces().times(1).returning(|| {
            Ok(vec![WorkspaceInfo {
                id: "1".into(),
                name: "w1".into(),
                created_at: 0,
                updated_at: 0,
            }])
        });
        api.expect_list_agents().times(1).returning(|_, _, _, _| {
            Ok(AgentPage {
                agents: vec![
                    agent_in_mode("analyst", "agentic_chat"),
                    agent_in_mode("future", "some_mode_invented_next_quarter"),
                    agent_in_mode("runner", "workflow"),
                    agent_in_mode("runner2", "workflow"),
                ],
                total: 4,
            })
        });
        let listing = collect_agents(&api, None, None, false, false, 1, 20)
            .await
            .unwrap();

        let hidden_modes = listing.hidden_modes.clone();
        let uids: Vec<_> = listing
            .agents_from_server()
            .iter()
            .map(|a| a.uid.clone())
            .collect();
        assert_eq!(uids, ["analyst"], "agentic_chat must be listed");

        assert_eq!(
            hidden_modes,
            BTreeMap::from([
                ("workflow".to_string(), 2),
                ("some_mode_invented_next_quarter".to_string(), 1),
            ]),
            "every withheld agent must be counted under its mode"
        );
    }

    /// One workspace with one agent of its own.
    fn one_workspace_api(agents: Vec<super::client::AgentInfo>) -> MockAgentApi {
        let mut api = MockAgentApi::new();
        api.expect_list_workspaces().times(1).returning(|| {
            Ok(vec![WorkspaceInfo {
                id: "1".into(),
                name: "w1".into(),
                created_at: 0,
                updated_at: 0,
            }])
        });
        let total = agents.len() as u32;
        api.expect_list_agents()
            .times(1)
            .returning(move |_, _, _, _| {
                Ok(AgentPage {
                    agents: agents.clone(),
                    total,
                })
            });
        api
    }

    #[tokio::test]
    async fn public_agents_are_appended_when_listing_every_workspace() {
        let listing = collect_agents(
            &one_workspace_api(vec![agent("mine", true)]),
            None,
            None,
            false,
            false,
            1,
            20,
        )
        .await
        .unwrap();
        let uids: Vec<_> = listing.agents.iter().map(|a| a.uid.as_str()).collect();
        assert_eq!(uids, ["mine", "chatbot"], "public agent must be listed");
        let public = listing.agents.last().unwrap();
        assert_eq!(public.workspace_id, PUBLIC_WORKSPACE_LABEL);
        assert!(public.is_published);
        assert!(
            is_chat_capable(&public.mode),
            "must survive the mode filter"
        );
    }

    #[tokio::test]
    async fn a_public_agent_you_own_is_not_duplicated() {
        // Whoever owns workspace 33 gets `chatbot` from the server already.
        let mut real = agent("chatbot", true);
        real.name = "LongbridgeAI".into();
        let listing = collect_agents(
            &one_workspace_api(vec![real]),
            None,
            None,
            false,
            false,
            1,
            20,
        )
        .await
        .unwrap();
        assert_eq!(
            listing.agents.len(),
            1,
            "the server record must not be shadowed by the stub"
        );
        assert_ne!(listing.agents[0].workspace_id, PUBLIC_WORKSPACE_LABEL);
    }

    #[tokio::test]
    async fn workspace_scoped_listing_omits_public_agents() {
        let mut api = MockAgentApi::new();
        api.expect_list_agents().times(1).returning(|_, _, _, _| {
            Ok(AgentPage {
                agents: vec![agent("mine", true)],
                total: 1,
            })
        });
        let listing = collect_agents(&api, Some("1".into()), None, false, false, 1, 20)
            .await
            .unwrap();
        let uids: Vec<_> = listing.agents.iter().map(|a| a.uid.as_str()).collect();
        assert_eq!(uids, ["mine"], "--workspace asks about one workspace only");
    }

    #[tokio::test]
    async fn name_filter_also_applies_to_public_agents() {
        let listing = collect_agents(
            &one_workspace_api(vec![]),
            None,
            Some("longbridge".into()),
            false,
            false,
            1,
            20,
        )
        .await
        .unwrap();
        assert_eq!(listing.agents.len(), 1, "case-insensitive name hit");

        let listing = collect_agents(
            &one_workspace_api(vec![]),
            None,
            Some("no-such-agent".into()),
            false,
            false,
            1,
            20,
        )
        .await
        .unwrap();
        assert!(
            listing.agents.is_empty(),
            "non-matching name must exclude it"
        );
    }

    #[tokio::test]
    async fn nothing_hidden_means_nothing_to_report() {
        let listing = collect_agents(&mixed_mode_api(), None, None, false, true, 1, 20)
            .await
            .unwrap();
        assert!(
            listing.hidden_modes.is_empty(),
            "--all hides nothing, so there is nothing to warn about"
        );
    }

    #[test]
    fn hidden_mode_label_is_sanitized_flattened_and_capped() {
        // The mode is server-supplied and lands on stderr, so it gets the
        // same treatment the table gives it.
        assert!(!render_mode_label("work\x1b[31mflow").contains('\x1b'));
        assert_eq!(render_mode_label("a\nb\tc"), "a b c");
        assert_eq!(render_mode_label("   "), "<empty>");
        let long = render_mode_label(&"x".repeat(200));
        assert!(long.chars().count() <= 41, "not capped: {long}");
        // Multi-byte input must not be cut mid-character.
        let cjk = render_mode_label(&"模式".repeat(50));
        assert!(cjk.ends_with('…'));
    }

    #[test]
    fn chat_capability_is_an_allowlist() {
        assert!(is_chat_capable("chat"));
        assert!(is_chat_capable("agentic_chat"));
        assert!(!is_chat_capable("workflow"));
        // An unknown mode is withheld — safe only because `hidden_modes`
        // forces the caller to say so. See the test below.
        assert!(!is_chat_capable("some_mode_invented_next_quarter"));
    }

    #[tokio::test]
    async fn collect_agents_all_modes_reveals_workflow() {
        let agents = collect_agents(&mixed_mode_api(), None, None, false, true, 1, 20)
            .await
            .unwrap()
            .agents_from_server();
        let uids: Vec<_> = agents.iter().map(|a| a.uid.as_str()).collect();
        assert_eq!(uids, ["talker", "runner"], "--all must hide nothing");
    }

    #[tokio::test]
    async fn mode_filter_composes_with_published_filter() {
        let mut api = MockAgentApi::new();
        api.expect_list_workspaces().times(1).returning(|| {
            Ok(vec![WorkspaceInfo {
                id: "1".into(),
                name: "w1".into(),
                created_at: 0,
                updated_at: 0,
            }])
        });
        api.expect_list_agents().times(1).returning(|_, _, _, _| {
            Ok(AgentPage {
                agents: vec![
                    super::client::AgentInfo {
                        is_published: false,
                        ..agent_in_mode("draft-chat", "chat")
                    },
                    agent_in_mode("live-chat", "chat"),
                    agent_in_mode("live-workflow", "workflow"),
                ],
                total: 3,
            })
        });
        let agents = collect_agents(&api, None, None, true, false, 1, 20)
            .await
            .unwrap()
            .agents_from_server();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].uid, "live-chat");
    }
}
