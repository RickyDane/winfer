# winfer — manual test checklist

Release build: `src-tauri/target/release/bundle/macos/winfer.app` (or the `.dmg` next to it).
The build is unsigned (ad-hoc). App data lives in `~/Library/Application Support/dev.winfer.app/`.

Tick what works; note anything that doesn't (what you did, what you expected, what happened).

## 1. Basics
- [ ] App opens; window has the sidebar, a `home` workspace and the browser panel.
- [ ] Sidebar **+** → pick a git repo folder → a new workspace appears and is selected.
- [ ] Right-click a workspace → small menu (Open, Reveal folder, Remove workspace…); removing asks first.
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

- [ ] Drag the gap left of the browser panel to resize it (double-click resets); the width survives a restart.
- [ ] Sun/moon button next to the gear switches light/dark mode, terminals included; the choice survives a restart.

## 3. Agents, status and worktrees
- [ ] An agent opened in a git repo shows a branch badge (`claude-1`); its terminal starts with a "worktree …" line.
- [ ] Send Claude a prompt: its status dot turns green (working), then blue (done) when it stops.
- [ ] While you're in another app, an agent finishing or asking permission shows a macOS notification.
- [ ] ⌘J jumps to the agent that needs you.
- [ ] Click the branch badge: diff view shows the agent's changes; Merge works once the agent committed; Discard removes the worktree.
- [ ] ⌘. interrupts the focused agent.

## 4. Orchestration
- [ ] ⌘⇧I broadcast: message goes to the selected agents.
- [ ] ⌘⇧R race: enter a task, pick 2+ agents, Start → each runs in its own worktree; the board shows their changes; Pick merges the winner and removes the others.
- [ ] Ask Claude: "use the winfer tools to list the panes" → it calls `list_panes` (it will ask permission). Try "start a codex agent and tell it to …".
- [ ] Recipes: ⌘K → "Recipe: Full stack" sets up agents + layout + localhost:3000; "Recipes…" → save the current layout under a name.
- [ ] Sidebar "Latest" shows what happened.
- [ ] Review inbox (sidebar "Review", ⌘E): when an agent finishes work in its worktree, a card appears with a summary, changed files, "Run tests" and (if it started a dev server) a screenshot. Merge commits leftovers and merges; then "Close agent and delete worktree". Send back types your comment into the agent. Race cards show "Pick".
- [ ] Say "what's done?" → the inbox opens and the summaries are read aloud.
- [ ] Give an agent a task: a thin bar under its pane header and a percent fill in within about a minute and only move forward; the sidebar shows "N working · X%"; Sessions (⌘O) lists every agent with task, bar and current step. Finishing jumps to 100%; a new task starts at 0%.
- [ ] Say "how far are the agents?" → the overview opens and progress is read aloud.

## 5. Browser
- [ ] Address bar: `github.com` loads; clicking, scrolling and typing in the page work; back/forward/reload.
- [ ] Phone icon toggles mobile view.
- [ ] Run a dev server in a terminal (e.g. `npm run dev`): a `localhost:5173` chip appears in the pane header; clicking it opens the preview.
- [ ] Crosshair (⌘⇧C): hover highlights elements; click → element details are pasted into the agent's prompt (not sent).
- [ ] Console icon shows a red count when the page logs errors; "Send errors to agent" pastes them.
- [ ] Ask Claude to open a page with its browser (Playwright MCP): the tab appears with an **agent** badge.

## 6. Voice
- [ ] Settings (⌘,): base.en speech model shows Installed (download it if not).
- [ ] Settings → Microphone lists your devices (MacBook Pro Microphone, AirPods…); pick one → Test → "Heard you on …" with a moving meter. "System default" follows macOS.
- [ ] Enter Cloudflare account ID + Workers AI token → Save → Test connection shows a latency.
- [ ] Hold ⌥Space, say "open two new instances with Claude and Codex", release → two panes open. (First use asks for microphone access.)
- [ ] "Create a new Claude instance and tell it to create a Flappy Bird clone" → Claude opens with that prompt.
- [ ] "Type hello world into Claude" → text is typed into Claude's prompt, not sent.
- [ ] Prompt writer (Settings, Workers AI by default): "tell Claude to make the page nicer" → Claude receives a concrete prompt (shown in the HUD). Try "OpenAI via your ChatGPT plan" too.
- [ ] After an agent created a page: "Öffne die erstellte Seite im Browser" / "open the page you just made" opens that file; "show me the dev server" opens localhost.
- [ ] "Close all windows" closes every pane in the workspace.
- [ ] With several agents open: "Sag allen, sie sollen einen Flappy Bird Klon erstellen, bis auf das letzte Fenster, das soll einen Super Mario Klon erstellen" → each pane gets its own prompt (HUD: "Writing 4 prompts…").
- [ ] Hold ⌥⇧Space and dictate → text is typed into the focused pane, not sent.
- [ ] Commands run immediately (no confirmation). Optional: Settings → "Ask before running uncertain or risky voice commands" brings back the confirm prompt.
- [ ] Settings → Language "Deutsch" + download a multilingual model → "Öffne ein neues Fenster mit Codex" works.
- [ ] "Open Claude with Opus" / "Switch Claude to Sonnet"; pane header shows the model; pane menu → Change model….
- [ ] Optional: enable "Speak agent updates" and let an agent finish while you're elsewhere.
- [ ] Optional: `CF_ACCOUNT_ID=… CF_API_TOKEN=… pnpm eval:voice` reports accuracy (target ≥ 90%) and latency.

## 7. Windows
- [ ] Installed CLIs (Claude Code, Codex via npm, …) show as installed in the "New" menu and start in a pane.
- [ ] Shortcuts use Ctrl+Shift (Ctrl+Shift+T new pane, Ctrl+Shift+K palette, Ctrl+Shift+/ shortcuts); tooltips and the empty screen show the same keys.
- [ ] Hold Alt+Space to talk; if another app owns it, Settings says so and the mic button still works.

## Known gaps
- Unsigned build; no auto-update yet.
- Windows untested; status hooks there are unlikely to work yet.
- Browser tabs aren't restored after restart; IME/composed input in the browser isn't supported.
- No undo for voice actions.
