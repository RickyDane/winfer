# Voice orchestrator (native-gpui)

**Status: built** (native only). Main code: `native/crates/winfer-app/src/ui/voice_orchestrator.rs`.
Differences from the plan below:
- No change to `src-tauri/src/mcp.rs`. Orchestrators get winfer's UI actions through the existing
  `__plugin_tools` list, which the app extends for orchestrator pane ids (`agent_api.rs`).
- Voice keeps its quick browser tools (`use_browser`, `web_*`) and `run_in_background`, and also
  gets `orchestrate`. Longer page tasks go to the orchestrator.
- A UI action from an orchestrator whose workspace isn't on screen switches the view to that
  workspace first.
- Debug aid: `WINFER_NATIVE_ORCHESTRATE="…"` hands it a request at launch and prints the answer.

Goal: live voice works as the main agent. "What's the latest change?", "Have some agents build xyz",
"Is the API agent done? Merge it": everything the user could look into or do by hand, voice does.

Decisions (from the user): **native-gpui only** · brain = **hidden `winfer code --serve` session** ·
shown **only in the voice HUD** · **fully autonomous** (merge, discard and close still need a spoken yes).

## Why it can't do this today

| Need | Today |
|---|---|
| Look into the repo (git log/diff, files, search, test output) | No git, file or shell-output tools; `bg` only starts long-running commands |
| Answers from winfer's own data (diffs, review inbox, history, chats) | `show_changes`, `review_work`, `show_history` open UI and return "Done." |
| Plan and split work | `gpt-realtime` is weak at planning; `spawn_panes` gives every pane the same prompt |
| Follow agents to the end, review, follow up | Gets "[winfer] … finished" notes, but can't keep track of several agents over time |
| Many tools | A long tool list already makes OpenAI sessions fail (`voice_ui.rs` retries with fewer tools) |

## Architecture

```
 you ⇄ realtime voice model (ears, mouth, quick UI actions)
            │ orchestrate(request)                    ▲ "[winfer] Orchestrator: <answer>"
            ▼                                         │
   hidden winfer code --serve session (one per workspace, cwd = workspace root, auto-approve)
     • its own tools: shell, git, read/search/edit files
     • winfer MCP: list/read/send/spawn panes, chats, bg, browser, visualize, … + winfer_action
            │ spawn_agent / send_to_pane / read_pane        ▲ "[winfer] pane 3 (Claude) finished"
            ▼                                               │
         worker agent panes (visible, usually in worktrees) ┘
```

- **Voice** keeps the actions that should be instant (focus, switch workspace, open URL/file,
  approve, interrupt, list/read pane, layout, end conversation) and gets one new tool,
  `orchestrate { request, fresh? }`. Everything else goes to the orchestrator: questions about
  code or history, multi-step work, browser use, background commands. That makes the voice tool
  list smaller, which also helps with the OpenAI failures.
- **`orchestrate` returns immediately** ("Handed over."). Voice says "On it" and keeps listening.
  When the orchestrator's turn ends (`Ev::TurnDone`), its last `Ev::Text` becomes a voice note.
  Voice speaks it the next time it's free; the client already queues notes.
- **Orchestrator** = a `CodeModel` + `Session` like a code pane, but with a made-up pane id
  (`voice-orch-<workspaceId>`), kept out of every workspace's panes/layout. Backend and model come
  from Settings → Voice → Orchestrator (default: winfer code's default). `autoApprove: true`,
  `systemPrompt` = `assets/orchestrator-role.txt`. It stays alive for the app run, so follow-ups
  ("and the other one?") have context. `fresh: true` (or "start over") restarts it.
- **Autonomous monitoring**: panes the orchestrator spawns are remembered as its own. When one of
  them finishes, needs input or fails, a `[winfer] …` line is typed into the orchestrator. While
  it's working, these wait in a queue (the same pattern as `deliver_bg_notes`). It reads the pane,
  checks the worktree diff, sends follow-ups or tells voice. It reports only what matters.
- **Confirmation**: `winfer_action` for merge, discard, close and stop-all answers "ask the user
  first". The orchestrator ends its turn with the question, voice asks it aloud, and the user's
  "yes" goes back through `orchestrate`.
- **HUD**: shows the orchestrator's current step from `Ev::ToolStart` titles (e.g.
  `Orchestrator · git log -5`), plus a busy dot when it's working with voice idle.
- **Voice off**: the orchestrator keeps working. Its answers go to a toast plus a desktop
  notification.

## Changes

| File | Change |
|---|---|
| `native/crates/winfer-app/src/ui/voice_orchestrator.rs` (new) | Lifecycle per workspace (start, restart, kill when the workspace closes), request queue, TurnDone → note/toast, ToolStart → HUD, which panes it owns, forwarding agent events |
| `native/crates/winfer-app/assets/orchestrator-role.txt` (new) | Role: you're the user's main agent; look into things yourself (git, files, panes, chats); give coding work to agents with standalone prompts in worktrees; follow them to the end; answer in 1–2 spoken sentences |
| `native/crates/winfer-app/assets/live-role.txt` | When to hand off versus act directly; read orchestrator notes aloud briefly |
| `ui/voice_tools.rs` | `orchestrate` tool; slimmer voice tool set; `registry_commands` reused by `winfer_action` |
| `ui/voice_ui.rs` | Handle `orchestrate`; HUD step line; keep the orchestrator alive on stop |
| `ui/agent_api.rs` | `run_tool` accepts the orchestrator's pane id (resolves to its workspace, like `run_voice_agent_tool`); `spawn_agent` records the owner; `winfer_action` runs `registry_commands` with the confirmation check |
| `src-tauri/src/mcp.rs` (shared backend) | Add the `winfer_action` tool, hidden for every pane except orchestrators (existing `HIDE_ENV` filter) |
| Where `voice_agent_note` is called | Also forward to the owning orchestrator |
| `ui/settings_more.rs` | Voice page → Orchestrator: backend/model, "Start fresh" |
| Tests | Pane-id routing, owner tracking, note queueing while busy, TurnDone → answer text, confirmation gate, tool set slimming |

## Risks / open points

- **Latency**: an orchestrator turn takes 5–60 s. Voice says so up front and stays usable meanwhile.
- **Cost**: runs on the chosen backend's plan (Claude, Codex or ChatGPT), like any agent pane.
- **Hooks** for the made-up pane id: `status::report` ignores unknown panes, which is fine. Check
  the turn, approval and session hooks before relying on that elsewhere.
- **Realtime notes** arriving while the user is speaking: check that the native client flushes
  queued notes only when no response is active (`voice/client.rs`).
