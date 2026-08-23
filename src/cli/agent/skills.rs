//! Static skill document served by `longbridge agent --skill`.

use indoc::indoc;

pub fn skills_doc() -> &'static str {
    indoc! {r#"
        # LongbridgeAI Agent Chat

        Chat with LongbridgeAI agents (investment research, stock analysis,
        screeners, custom workflow agents) from the command line. Designed for
        AI harnesses: every command supports `--format json`.

        ## 1. Discover agents

        ```bash
        longbridge agent list --format json          # all workspaces, merged
        longbridge agent list --name screener --format json
        ```

        Only agents with `"is_published": true` can chat. Note the `uid`.

        The list shows conversational agents only — `mode` of `chat` or
        `agentic_chat`, which behave identically here. Other modes (e.g.
        `workflow`) are hidden because `agent chat` cannot drive them;
        chatting with one fails with a bare `status=failed`.

        The listing merges two sources: agents in workspaces you own, and the
        platform's public catalog (every published, publicly shared agent —
        e.g. `chatbot`, LongbridgeAI's general investment research assistant).
        Public agents appear with `"workspace_id": "Public: Longbridge"`.

        Any published agent is chattable by uid, so if you are handed a uid
        that is not in the list, still try it.

        Whenever something is hidden, a `note:` line naming the modes and
        counts goes to **stderr** (never stdout, so `--format json` stays
        parseable), and `--all` lists everything.

        ## 2. Start a conversation

        ```bash
        longbridge agent chat <AGENT_UID> "your question" --format json
        ```

        The command streams server-sent events internally and prints one JSON
        object when the run completes (runs can take 1-2 minutes):

        ```json
        {
          "chat_uid": "…",          // conversation id — keep it for follow-ups
          "message_id": "…",        // this round's message id
          "status": "succeeded",    // succeeded | interrupted | failed | stopped | unknown
          "answer": "…markdown…",
          "widgets": [
            {"kind": "vis-chart", "spec": {"type": "column"}},
            {"kind": "x-widget", "src": "widget://quote/security/detail?symbol=TSLA.US"}
          ],
          "references": [],          // cited sources
          "further_questions": [],   // suggested follow-ups
          "elapsed_time": 42.0
        }
        ```

        The `answer` is markdown. Embedded `vis-chart` code fences and
        `<x-widget>` tags are additionally extracted into `widgets` so you do
        not need to parse them yourself.

        ## 3. Multi-turn follow-ups

        Pass the previous round's ids back (positional or flag form):

        ```bash
        longbridge agent chat <AGENT_UID> <CHAT_UID> <MESSAGE_ID> "follow-up question"
        longbridge agent chat <AGENT_UID> "follow-up" --chat-uid <CHAT_UID> --parent-message-id <MESSAGE_ID>
        ```

        ## 4. Interrupted runs

        When `status` is `interrupted`, the agent asked clarifying questions
        (in `interrupt.questions`, keyed by `interrupt.tool_call_id`).

        Each entry of `interrupt.questions` is an OBJECT, not a string:

        ```json
        {
          "question": "Which symbol should I back-test?",
          "multi_select": false,
          "options": [{"description": "TQQQ — 3x long Nasdaq"}]
        }
        ```

        The answer key must be the inner `question` value — not the whole
        object. When `options` is non-empty, prefer one of their `description`
        values. Extract it with `jq -r '.interrupt.questions[].question'`.

        Answer with `agent continue` — NOT with a new `chat` call:

        ```bash
        longbridge agent continue <AGENT_UID> <CHAT_UID> <MESSAGE_ID> \
          --answer "<tool_call_id>:<question>=<your answer>" --format json
        ```

        A bare `--answer "<value>"` also works when the CLI has a cached copy
        of the interrupt (same machine, single question).

        If a question itself contains `=`, skip the `--answer` grammar and
        pass the raw payload instead — no parsing ambiguity:

        ```bash
        longbridge agent continue <AGENT_UID> <CHAT_UID> <MESSAGE_ID> \
          --answers-json '{"<tool_call_id>":{"Is P/E=20 acceptable?":"yes"}}' --format json
        ```

        ## 5. Exit status

        Exit code 0 means either `status: "succeeded"` or `status:
        "interrupted"` with an `interrupt` payload to answer. Every other
        status exits non-zero, including `failed`, `stopped`, and
        `status: "unknown"` — the latter means the SSE stream was dropped
        before the run finished (network blip, proxy timeout, etc.). Any
        `answer` text received so far is still printed / included in the JSON
        object; check `error_message` and consider retrying the same query.

        ## 6. Etiquette

        - Requires OAuth login (`longbridge auth login`). API-key env
          authentication is not supported for AI conversations.
        - Serialize requests; the API rejects bursts with code 429002.
          On 429002, wait a few seconds and retry.
        - Agent runs are slow (up to ~2 min). Do not re-send while running.
        - `longbridge agent chat --schema` prints the response field schema.
    "#}
}

pub fn print_skills_doc() {
    println!("{}", skills_doc());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_doc_covers_the_whole_flow() {
        let doc = skills_doc();
        for needle in [
            "longbridge agent list --format json",
            "longbridge agent chat",
            "longbridge agent continue",
            "--chat-uid",
            "--parent-message-id",
            "interrupted",
            "unknown",
            "further_questions",
            "429002",
            "vis-chart",
            "x-widget",
        ] {
            assert!(doc.contains(needle), "skills doc missing: {needle}");
        }
    }
}
