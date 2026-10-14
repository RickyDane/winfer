# winfer — manual test checklist

Release build: `src-tauri/target/release/bundle/macos/winfer.app` (or the `.dmg` next to it).
The build is unsigned (ad-hoc). App data lives in `~/Library/Application Support/dev.winfer.app/`.

Tick what works; note anything that doesn't (what you did, what you expected, what happened).

## 1. Basics
- [ ] App opens; window has the sidebar, a `home` workspace and the browser panel.
- [ ] Sidebar **+** → pick a git repo folder → a new workspace appears and is selected.
- [ ] Right-click a workspace → Open, New <default agent> here, Reveal, Copy path, Remove workspace… (asks first).
- [ ] Right-click an agent in the sidebar → Focus, Zoom, Show in overview, Approve (when waiting), Interrupt, Restart agent, Change model…, Show changes / Open its review / Merge…, Reveal, Copy path/branch, Discard worktree…, Close; each works.
- [ ] Right-click a worktree in the sidebar → Open <any installed agent> here / Open a shell here, Reveal, Copy path/branch, Remove worktree….
- [ ] ⌘T opens Claude Code in that workspace (or a shell if Claude isn't installed).
- [ ] "New" menu lists providers; uninstalled ones are greyed out.
- [ ] Typing in a terminal works; colors, links (click) and copy/paste (⌘C/⌘V) work.
- [ ] Quit (⌘Q) and reopen: workspaces, panes and layout come back; Claude/Codex resume their last session.

## 2. Tiling and keys (⌘/ shows all shortcuts)
- [ ] ⌘D / ⌘⇧D split right/down; ⌘W closes the focused pane (not the window).
- [ ] Drag a gap between panes to resize; ⌘⌃←→↑↓ resizes from the keyboard.
- [ ] ⌘⌥←→↑↓ moves focus; ⌘⇧⌥←→↑↓ swaps panes; ⌘1–9 focuses pane N.
- [ ] ⌘↩ zooms a pane and back; ⌘⌃L cycles layouts; ⌘⌃= equalizes.
- [ ] Drag a pane by its header onto another pane: edges place it beside (left/right/top/bottom), the middle swaps; Esc cancels.
- [ ] ⌘K palette: type "grid", Enter → grid layout.
- [ ] ⌘/ → click a shortcut → press new keys → it works right away and survives a restart; a taken chord offers "Use it here instead"; ↺ restores; ⌫ switches it off; "Reset all".
- [ ] Change the voice keys in the same sheet (Voice section) → holding the new keys records.
- [ ] Settings → New panes → Default agent (or the ★ in the New menu): ⌘T and "open a new window" start that agent.
- [ ] ⌘F searches a pane's scrollback (Enter / ⇧Enter to step).
- [ ] ⌘= / ⌘- / ⌘0 (Ctrl+Shift+= / - / 0 on Windows), ⌘/Ctrl + scroll or pinch over a terminal, or Settings → Terminals → Text size change every terminal's text; agents re-wrap to the new width; the size survives a restart. "Make the terminal text bigger" works by voice.

- [ ] Drag the gap left of the browser panel to resize it (double-click resets); the width survives a restart.
- [ ] Sun/moon button next to the gear switches light/dark mode, terminals included; the choice survives a restart.

- [ ] Title bar tabs: "<workspace>" | "Overview" (⌘⇧O / Ctrl+Shift+Alt+O). Overview tiles every session of every workspace like a workspace (same headers, gutters, zoom, drag and drop, ⌘1–9), each labeled with its workspace; rearranging it doesn't touch the workspaces' own layouts.
- [ ] In the overview, click a session from another workspace and press ⌘T → the new agent opens in that workspace. The bar on top messages / approves / interrupts all, working or waiting agents across workspaces.
- [ ] Workspace view: the bar above the panes ("Message N agents in <workspace>") reaches all, working or waiting agents of that workspace only; Approve / Interrupt too. It's hidden in workspaces without agents.
- [ ] Windows: the sidebar toggle sits at the left edge of the title bar (no gap for macOS window buttons).

## 3. Agents, status and worktrees
- [ ] An agent opened in a git repo shows a branch badge (`claude-1`); its terminal starts with a "worktree …" line.
- [ ] Send Claude a prompt: its status dot turns green (working), then blue (done) when it stops.
- [ ] While you're in another app, an agent finishing or asking permission shows a macOS notification.
- [ ] While you're in another app, Claude asking for permission shows a notification with its actual question (e.g. "Bash command · rm -rf dist · Do you want to proceed?") and Approve / Decline; each answers in the pane without switching to winfer. An open question gets a reply field (macOS) whose text is sent to the agent; clicking the notification opens that session. macOS: set winfer's notification style to "Alerts" (System Settings › Notifications) to keep the buttons on screen; with "Banners" they appear on hover.
- [ ] ⌘J jumps to the agent that needs you.
- [ ] Sidebar agent rows show a thin progress bar under the name (with the percent on the right).
- [ ] Worktree rows show how old they are (last commit, else folder), "N ahead", "uncommitted", or "✓ merged" when nothing on them is missing from the base (safe to remove); merged ones are listed last; hover for details.
- [ ] Sidebar → "Branches" starts with the project folder itself (badge "project", its branch, age, uncommitted): clicking it opens an agent directly in the project folder (no worktree).
- [ ] "+" next to "Branches" (or right-click a workspace/branch → New worktree from a branch…, or the palette): search local and remote branches (Fetch updates them), pick one or start a new branch from it, then "Open <agent> in a worktree" or "New workspace from it" (a workspace rooted in that worktree, worktrees off). Branches checked out elsewhere say where.
- [ ] Sidebar → "Worktrees" under the active workspace lists the repo's worktrees no pane uses (closed agents' and hand-made ones, e.g. `git worktree add ../x -b x`); clicking one (or ▶) opens the default agent inside it with diff/merge/review working; 🗑 removes it (winfer's own branches are deleted too, others kept). New worktrees show up within 20 s or when winfer gets focus.
- [ ] Click the branch badge: diff view shows the agent's changes; Merge works once the agent committed; Discard removes the worktree.
- [ ] ⌘. interrupts the focused agent.

## 4. Orchestration
- [ ] ⌘⇧I broadcast: message goes to the selected agents.
- [ ] ⌘⇧R race: enter a task, pick 2+ agents, Start → each runs in its own worktree; the board shows their changes; Pick merges the winner and removes the others.
- [ ] Ask Claude: "use the winfer tools to list the panes" → it calls `list_panes` (it will ask permission). Try "start a codex agent and tell it to …".
- [ ] Recipes: ⌘K → "Recipe: Full stack" sets up agents + layout + localhost:3000; "Recipes…" → save the current layout under a name.
- [ ] Sidebar "Latest" shows what happened.
- [ ] Review inbox (sidebar "Review", ⌘E): when an agent finishes work in its worktree, a card appears with a summary, changed files, "Run tests" and (if it started a dev server) a screenshot. Merge commits leftovers and merges; then "Close agent and delete worktree". Send back types your comment into the agent. Race cards show "Pick".
- [ ] History (sidebar, ⌘⇧H): every merge (review card, pane menu, voice, race winner) appears with title, summary, task, files, branch → base, +/− lines, tests and the merge commit (copy). Search matches all words in titles, summaries, tasks, branches and file names; switch between this workspace and all; entries survive restarts.
- [ ] Say "what's done?" → the inbox opens and the summaries are read aloud.
- [ ] Sidebar and pane headers show what each agent works on (Claude Code's terminal title, else the task), with its CLI's icon; hover shows the CLI name.
- [ ] Start OpenCode/Codex without a task: no progress bar or percent. Give a task: estimates appear; while it works the bar stays below 100; when it stops after finishing, it can reach 100.
- [ ] Give an agent a task: a thin bar under its pane header and a percent fill in within about a minute and only move forward; the sidebar shows "N working · X%"; Sessions (⌘O) lists every agent with task, bar and current step. Finishing jumps to 100%; a new task starts at 0%.
- [ ] Say "how far are the agents?" → the overview opens and progress is read aloud.

- [ ] In the Overview tab, Review shows cards from every workspace (with the workspace name), its sidebar count covers all workspaces, and History opens on "All workspaces"; in a workspace both stick to that workspace.

## 5. Browser
- [ ] Address bar: `github.com` loads; clicking, scrolling and typing in the page work; back/forward/reload.
- [ ] Phone icon toggles mobile view.
- [ ] Run a dev server in a terminal (e.g. `npm run dev`): a `localhost:5173` chip appears in the pane header; clicking it opens the preview.
- [ ] Crosshair (⌘⇧C): hover highlights elements; click → element details are pasted into the agent's prompt (not sent).
- [ ] Console icon shows a red count when the page logs errors; "Send errors to agent" pastes them.
- [ ] Ask Claude to open a page with its browser (Playwright MCP): the tab appears with an **agent** badge.
- [ ] Android tab (next to Browser): with no adb installed it says how to get it; with a phone attached (USB debugging on, authorized) or an emulator running, it appears in the list and its screen mirrors live.
- [ ] Click taps, drag swipes, scroll wheel scrolls, typing goes to the focused field; Back / Home / Recents / Power buttons work; Reconnect restarts the stream.
- [ ] An emulator that isn't running shows a Start button that boots it.

- [ ] Terminal tab (or Ctrl+`): a shell in the workspace folder; it keeps running when switching tabs or workspaces (each workspace has its own); the ↻ button restarts it. Ctrl+` again hides the panel.

- [ ] OpenCode pane: ask it to open a page and click something / take a screenshot (tools `winfer-browser_*`), and to "notify me when you're done" (`winfer_notify_user`); a native notification appears. Works with OpenCode 1.x and 2.x (2.x is started with `--standalone`).

## 6. Voice
- [ ] Settings (⌘,): base.en speech model shows Installed (download it if not).
- [ ] Settings → Microphone lists your devices (MacBook Pro Microphone, AirPods…); pick one → Test → "Heard you on …" with a moving meter. "System default" follows macOS.
- [ ] Settings → Voice → "Continue with ChatGPT" → the browser asks you to sign in, name the agent and allow winfer to use your plan → back in winfer: "Signed in as you@…". Test connection shows a latency. On a ChatGPT plan it adds that the plan can't use the Decisions API (OpenAI refuses that route for plan tokens), so every voice command is planned by the LLM; this is remembered across restarts. The LLM model picker lists your account's models (gpt-6-luna, gpt-6-sol, …). Sign out → voice commands ask you to sign in again; signing in again skips the naming step. Declining in the browser → "You declined the sign-in."
- [ ] Restart winfer → still signed in. After an hour of use the token renews silently (no sign-in prompt).
- [ ] Hold ⌥Space, say "open two new instances with Claude and Codex", release → two panes open. (First use asks for microphone access.)
- [ ] "Create a new Claude instance and tell it to create a Flappy Bird clone" → Claude opens with that prompt.
- [ ] "Type hello world into Claude" → text is typed into Claude's prompt, not sent.
- [ ] LLM (Settings → Voice): ChatGPT once signed in (Claude Code if not; an option is greyed out until it's available). "tell Claude to make the page nicer" → Claude receives a concrete prompt (shown in the HUD). Pick another ChatGPT model, then switch to Claude Code (type a model like "haiku") and repeat. Off → spoken words go through unchanged, progress estimates and review summaries are off.
- [ ] Long or complex commands are planned by the LLM (HUD "Planning…"): "Open Codex and have it fix the login bug where the session expires too early, then run the auth tests" → one new Codex pane with one prompt; "Tell the Claude pane to stop and write unit tests for the payment service, and ask Codex to answer yes" → stops Claude, sends both messages. Short commands ("open two codex panes") still go to Clef. Settings → "Who decides what runs" switches between auto, always LLM and always Clef.
- [ ] Optional: `PLANNER=claude pnpm eval:planner` (or `PLANNER=openai OPENAI_TOKEN=…`, `PLANNER_MODEL=…`) checks the planner on long commands; `OPENAI_TOKEN=… pnpm eval:voice` checks the Decisions API on short ones.
- [ ] Several features for one new agent ("Open Claude: create credentials.js with placeholders, implement decide() in openaiDecisions.js and make Luna.vue use it") → one pane per feature, each in its own worktree with a standalone prompt; the HUD says "split into 3 tasks". Same with Codex, OpenCode or no agent named (default agent). One feature with follow-up steps ("fix the login bug, then run the tests") stays in one pane.
- [ ] Features without "open" ("We need four new features: a plugin system, a graphify plugin, an encrypted secret store and the OpenAI decisions API") → one default-agent pane per feature, each with its prompt, even while other agents are open. "Tell Claude to run the tests" still goes to the open Claude.
- [ ] After an agent created a page: "Öffne die erstellte Seite im Browser" / "open the page you just made" opens that file; "show me the dev server" opens localhost.
- [ ] "Close all windows" closes every pane in the workspace.
- [ ] With several agents open: "Sag allen, sie sollen einen Flappy Bird Klon erstellen, bis auf das letzte Fenster, das soll einen Super Mario Klon erstellen" → each pane gets its own prompt (HUD: "Writing 4 prompts…").
- [ ] Hold ⌥⇧Space and dictate → text is typed into the focused pane, not sent.
- [ ] Commands run immediately (no confirmation). Optional: Settings → "Ask before running uncertain or risky voice commands" brings back the confirm prompt.
- [ ] Settings → Language "Deutsch" + download a multilingual model → "Öffne ein neues Fenster mit Codex" works.
- [ ] "Open Claude with Opus" / "Switch Claude to Sonnet"; pane header shows the model; pane menu → Change model….
- [ ] Optional: enable "Speak agent updates" and let an agent finish while you're elsewhere.

## 6a. Usage limits
- [ ] After a Claude Code pane in winfer answers once, a ring with the Claude icon appears in the title bar; your own Claude status line still shows in the pane.
- [ ] After a Codex turn, a Codex ring appears (read from ~/.codex/sessions). Rings drain with use: green, amber under 30% left, red under 10%.
- [ ] Click the rings: 5-hour and weekly bars per agent, reset times, "at this pace runs out …" once there's ≥15 min of history; OpenCode shows today's tokens and cost.
- [ ] The New menu shows "% left" next to Claude Code / Codex; pane headers show only the context bar, not the usage limits.
- [ ] Crossing 80% / 95% used shows a toast (native notification when winfer is in the background); at 100% the toast offers "Continue in Codex" (or the agent with most room), which opens it next to the pane in the same worktree with the task and last output.

- [ ] Context: Claude Code, Codex and OpenCode pane headers show a small bar with how full the context window is (number hidden in narrow panes; tooltip "84k of 200k tokens"); amber from 60%, red from 85%. Claude updates with its status line, Codex after each turn (and every 30 s while working), OpenCode 2.x every 20 s (from its session database).

## 6b. Feedback and motion
- [ ] Dialogs (Settings, palette, review, history…) fade in; menus pop in; new panes grow in; switching workspace or Overview fades the content.
- [ ] A new agent pane shows "Creating worktree…" then "Starting Claude Code…" until its first output.
- [ ] Merge / discard / remove worktree show a toast with a spinner, then "Merged …" / "Removed …" (bottom right). Copy path / branch / commit SHA shows "Copied".
- [ ] Waiting-for-input dots pulse a ring; a sidebar row flashes when its agent finishes; progress bars sweep while estimating.
- [ ] Spinners on: diff loading and refresh, sessions refresh, settings Save / Test connection, "Starting browser…", worktree being removed; history shows placeholder cards while searching; review cards show a placeholder summary.
- [ ] macOS "Reduce motion" turns the animations off (spinners keep turning).

## 7. Windows
- [ ] Installed CLIs (Claude Code, Codex via npm, …) show as installed in the "New" menu and start in a pane.
- [ ] Shortcuts use Ctrl+Shift (Ctrl+Shift+T new pane, Ctrl+Shift+K palette, Ctrl+Shift+/ shortcuts); tooltips and the empty screen show the same keys.
- [ ] Hold Alt+Space to talk; if another app owns it, Settings says so and the mic button still works.

## Known gaps
- Unsigned build; no auto-update yet.
- Windows untested; status hooks there are unlikely to work yet.
- Browser tabs aren't restored after restart; IME/composed input in the browser isn't supported.
- No undo for voice actions.

## Plugins
- [ ] Copy `docs/plugins/example-hello` into `~/Library/Application Support/dev.winfer.app/plugins/` → Settings → Plugins → Reload plugins → it's listed, off, with "1 tool · 1 agent · 1 recipe · code".
- [ ] Turn it on → asks first → "Run tests", "Run lint", "Count agents by status" and "Recipe: Agent + tests" appear in ⌘K and work; Gemini CLI appears in the New menu (greyed out if not installed).
- [ ] Say "run the linter" → runs the plugin tool.
- [ ] With it on, an agent finishing shows "<agent> finished".
- [ ] Turn it off → its palette entries, agent and recipe are gone. A plugin with a broken plugin.json shows the error and can't be turned on.
