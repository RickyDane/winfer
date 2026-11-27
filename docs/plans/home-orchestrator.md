# Home: a global orchestrator above all workspaces (native-gpui)

**Status: built** (native only, branch `home-orchestrator`). Main code: `native/crates/winfer-app/src/ui/home.rs`
(engine, tasks, tools) and `ui/home_view.rs` (the view). Roles: `assets/conductor-role.txt`, `assets/task-role.txt`.
Differences from the design below:
- Settings → Home → Task agent picks what tasks start (default WinferCode; "Orchestrator picks" leaves it to them). read_pane returns a WinferCode pane's transcript.
- Models per task: start_task takes model/effort/reason, set_task_model switches a task's agents
  mid-run (WinferCode hands the conversation to the new model, across providers), list_models shows
  what's available with usage left. Remote task orchestrators get list_models/set_model on winferd.
  How to choose is a hidden guide in every orchestrator's system prompt
  (`assets/model-routing.md`; `<data>/home/model-routing.md` overrides it). The card shows the model.
- The side panel shows only the task cards (needs you, reviews, workspaces and usage were removed).
- Shortcut is ⌘⇧M (⌘0 resets the text size). Also in the palette, and first in the sidebar and rail.
- "Chips when unsure" are the conductor's `ask_user` picker in Home's chat (typed requests only).
- Inline cards are clickable links in its replies: `winfer://task/3`, `winfer://pane/api/2`,
  `winfer://workspace/api`.
- Remote task orchestrators are shown on the server as panes titled "Task N · goal" (auto-approve and
  the task role come from that title), not hidden. Home finds them by that title in the mirror.
- The phone shows Home pinned in its own section on the Agents page, outside the counts.
- `usage_status` from Home's orchestrators lists every account.
- Debug aid: `WINFER_NATIVE_ORCHESTRATE="…"` still hands a request at launch (now to the conductor).

Goal: one place you can open and just say what you want ("fix the flaky relay test", "what are my
agents doing?", "merge everything that passes") without picking a workspace or opening a winfer
code pane first. It sees and acts on every workspace, session and tool on every paired machine.

## Decisions so far

- **native-gpui only**, built on the voice orchestrator engine (`ui/voice_orchestrator.rs`).
- **One conductor plus task forks**: one always-on Home thread, and each larger task runs in its
  own sub-orchestrator thread.
- **Voice merges into Home**: voice feeds the same conductor. Per-workspace orchestrators
  (`voice-orch-<ws>`) go away.
- **Autonomy**: it acts on its own (starts agents, sends follow-ups, runs tests) and asks before
  merge, discard, close or stop-all, the same rule as today.
- **Remote tasks run on the remote machine**: the fork is a `winfer` session on the server
  (winferd), so it keeps going while the laptop sleeps. It reports to the conductor over Connect.
- **Every coding job is a task; a fork only when needed** (see "Tasks" below).
- **Target workspace**: it guesses from the request and recent activity, and shows workspace chips
  only when it isn't sure.
- **Models**: the conductor and the task forks each have their own backend/model setting
  (Settings → Home).
- **No scheduling** (standing orders) for now.
- **Mobile**: supports Home, but still opens on the agent page as today.

## Shape

```
                 you (typing in Home · voice · phone)
                              │
                 ┌────────────▼────────────┐
                 │  Conductor (home-orch)  │  cwd: ~/.winfer/home, one per app
                 │  overview, routing,     │  answers quick questions itself
                 │  quick actions          │
                 └──┬──────────┬──────────┬┘
       start_task   │          │          │  "[winfer] Task 3: done, 2 commits, tests pass"
                    ▼          ▼          ▼
            Task fork A   Task fork B   Task fork C      cwd = target workspace root
            (ws: api)     (ws: relay)   (mac-mini/web)   owns its worker panes
               │             │             │
          worker panes   worker panes  remote panes (via Connect)
```

- **Conductor**: a `CodeModel` like the voice orchestrator, with pane id `home-orch` and its own
  scratch folder as cwd. It answers quick questions directly (status, "what changed in api
  today", usage). For real work it calls `start_task { workspace, goal }`.
- **Task fork**: a sub-orchestrator `CodeModel` (`home-task-<id>`) with cwd = the target
  workspace's root. It inherits today's orchestrator role: look into things, split the work,
  start agents in worktrees, follow them to the end. It reports back to the conductor through a
  `[winfer] Task …` note, and you can open its thread and talk to it directly.
- **Remote forks** run as a hidden `winfer` session on the server, using the same task role.
  They spawn agents locally *on that server*. Status changes and "done" notes reach Home through
  Connect events. If the laptop is asleep, those events wait in a queue on the server and are
  delivered on reconnect.
- **Ownership** moves from "orchestrator per workspace" to "task owns panes". `Watch` and
  `owned` stay as they are, keyed by task.

## Tasks: lightweight by default, forked when needed

Every coding job gets a **task card** (goal, workspace, agents, stage), so tracking is the same
for everything. It doesn't always get its own orchestrator:

- **Light task** (default for one clear change: "bump the dep", "fix this test"): the
  conductor starts a single agent with a full prompt and watches that agent itself. No fork, no
  second model.
- **Forked task**: used when the work spans several agents or steps, runs long (needs checking
  and follow-ups), or runs on another machine (always a fork, on that machine).
- **Promotion**: a light task becomes a fork when it grows (the agent fails twice, asks design
  questions, or the user adds scope). The fork gets the task's history and takes over its panes.
- Questions, lookups and quick actions (approve, reply, merge after you say yes) never create a
  task.

This keeps the conductor's context small, because forks hold the long histories. It also avoids
paying for a second orchestrator on small jobs.

## Home UI

- A **Home** entry at the top of the sidebar (⌘0), and the default view on launch. It replaces
  `welcome.rs` when no workspace is open.
- **Center**: the conductor chat (reuses `code_view.rs`), with an input box that accepts
  `@workspace` / `@machine/workspace` mentions. When the target is ambiguous, it offers
  workspace chips to pick from.
- **Right rail** (live, no model involved):
  - **Needs you**: approvals, questions and errors, across every machine, with inline Approve /
    Reply buttons.
  - **Ready to review**: local worktrees plus `list_reviews` from servers, with Merge / Send back.
  - **Tasks**: one card per task fork, showing the goal, its stage (planning → running N agents
    → checking → ready → merged) and its workspace. Click a card to open its thread.
  - **Fleet**: machines → workspaces → agents, with their status.
  - **Usage**: what's left per account (`usage_status`, `machine_usage`).
- **Opening brief**: when you open Home after being away, the conductor posts a short summary
  ("2 agents waiting, 3 reviews ready, relay CI failed overnight").
- **Cards in replies**: pane, diff, review and task cards rendered inline, which you can act on
  or click to jump to.

## Voice

- Voice's `orchestrate` targets the conductor instead of the active workspace.
- Requests are tagged `User (via voice):` or `User (typed):`. The role tells it to answer
  voice requests in 1–3 spoken sentences and typed ones in markdown with cards.
- Pickers (`ask_user`) are allowed for typed requests now, because Home shows them. Voice keeps
  the rule to ask in the reply text.
- The voice HUD shows the step the conductor or the active task is on.

## Tool gaps (agent_api.rs / winfer MCP)

| Need | Today | Add |
|---|---|---|
| List all workspaces everywhere | `list_machines` + `list_remote_panes` only | `list_workspaces` (local + remote, roots, branches, live panes) |
| One address for any pane | local `3`, remote `mac-mini:3` | `[machine/]workspace/pane` accepted by read/send/approve/close |
| Spawn into any workspace | `spawn_agent` = caller's workspace | `spawn_agent { workspace }`, remote routes to `spawn_remote_agent` |
| Look into a repo without a pane | the orchestrator's own shell, limited to its cwd | the conductor uses `git -C`; task forks sit in the repo anyway |
| Start/track tasks | — | `start_task`, `list_tasks`, `task_say` (conductor only) |
| Remote pane events | polling `list_remote_panes` | Connect pushes status changes → `Watch` |
| Reviews from anywhere | `list_reviews` (servers), worktrees (local) | one merged list for the rail and the conductor |

## Persistence

- The task registry (id, goal, workspace, owned panes, stage, thread session id) lives in app
  state, so it survives restarts. Threads resume with winfer code's session resume.
- The conductor thread compacts over time. Each task keeps its full history in its own thread.

## Open questions

- winferd already runs `winferd code --serve` chat panes (`winferd/src/host.rs:763`,
  `chats.rs`), but as normal panes in a workspace. Remote forks need a hidden flavor: auto-approve,
  the task role, the forked-task tool set, and kept out of the pane list (or shown labeled as
  "Task").
- Does a remote fork's worktree merge happen on the server (`review_action merge`) or get pulled
  locally? Probably on the server, the way remote reviews work today.

## Rough build order

1. Global addressing + `list_workspaces` + `spawn_agent { workspace }` in `agent_api.rs`.
2. Conductor (`home-orch`): refactor `Orchestrators` from per-workspace to conductor + tasks.
3. Home view: sidebar entry, center chat, rail (Needs you, Reviews, Fleet) built from existing
   state.
4. Light tasks + Tasks rail + `start_task`/`list_tasks`.
5. Local forks + promotion of light tasks into forks.
6. Move voice over to the conductor; remove per-workspace orchestrators.
7. Remote: Connect status push, remote reviews in the rail, forks hosted on winferd.
8. Opening brief, inline cards, persistence across restarts, Settings → Home (two models).
9. Mobile: a Home tab (it still opens on the agent page).
