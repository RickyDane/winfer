"""Builds winfer voice-routing /v1/systemone requests for Clef-flash.

size:
  "xs"     ~550 tok : action_1 only, no option descriptions, minimal state
  "s"      ~1k tok  : action_1 (with descriptions) + target_pane, minimal state
  "full"   ~2.6k tok: the realistic spec schema (action_1/2, counts, target, resize, worktree, workspace)
  "large"  ~4k tok  : full schema + bigger app snapshot + descriptions on action_2
"""
import copy

TOOLS = {
    "spawn_panes": "Open new terminal panes running agents like claude, codex, agy, grok or a shell.",
    "close_pane": "Close an existing pane and stop whatever is running inside it.",
    "focus_pane": "Move keyboard focus to a specific existing pane.",
    "zoom_pane": "Maximize one pane to fill the window, or un-zoom it.",
    "resize_pane": "Make a pane bigger or smaller relative to its neighbours.",
    "swap_panes": "Swap the positions of two panes in the layout.",
    "equalize_panes": "Make all panes the same size.",
    "set_layout": "Change the pane arrangement, e.g. grid, columns, rows or stack.",
    "send_to_pane": "Type a message or instruction into one agent pane, e.g. tell claude to do something.",
    "broadcast": "Send the same message or instruction to every agent pane at once.",
    "approve_agent": "Approve or accept a pending permission request from an agent.",
    "interrupt_agent": "Stop or cancel what an agent is currently doing, without closing its pane.",
    "open_url": "Open a specific website, URL or localhost port in the built-in browser.",
    "browser_new_tab": "Open a new empty browser tab.",
    "browser_close_tab": "Close the current browser tab.",
    "browser_switch_tab": "Switch to another already-open browser tab.",
    "browser_back": "Go back to the previous page in the browser.",
    "browser_forward": "Go forward to the next page in the browser.",
    "browser_reload": "Reload or refresh the current browser page.",
    "set_device": "Change the microphone or audio input/output device.",
    "switch_workspace": "Switch to a different existing workspace or project.",
    "create_workspace": "Create a brand new workspace.",
    "toggle_sidebar": "Show or hide the sidebar.",
    "toggle_browser": "Show or hide the built-in browser panel.",
    "load_recipe": "Load a saved recipe, a preset set of panes and layout.",
    "show_shortcuts": "Show the list of keyboard shortcuts and voice commands.",
    "repeat_last": "Repeat the previous voice command again.",
    "undo_last": "Undo the previous voice command.",
    "none": "Not a command for the app: chatter, noise, unclear or unrelated speech.",
}

PROVIDERS = [
    {"id": "claude", "name": "Claude Code (Anthropic)"},
    {"id": "codex", "name": "Codex CLI (OpenAI)"},
    {"id": "agy", "name": "agy, Antigravity CLI (Google)"},
    {"id": "grok", "name": "Grok CLI (xAI)"},
    {"id": "shell", "name": "plain zsh shell"},
]

PANES = [
    {"id": "p1", "label": "Claude 1", "provider": "claude", "status": "waiting_for_approval"},
    {"id": "p2", "label": "Codex 1", "provider": "codex", "status": "running"},
    {"id": "p3", "label": "Shell", "provider": "shell", "status": "idle"},
    {"id": "p4", "label": "Grok 1", "provider": "grok", "status": "idle"},
]

WORKSPACES = ["winfer", "bridgemind-api", "bridgemind-web", "personal"]

TABS = [
    {"id": "t1", "title": "winfer dev - localhost:5173", "active": True},
    {"id": "t2", "title": "Pull requests - GitHub", "active": False},
]

RECENT = ["open two claudes and a codex", "focus pane one"]


def snapshot(size="full"):
    snap = {
        "workspace": "winfer",
        "workspaces": WORKSPACES,
        "focusedPane": "p2",
        "panes": PANES,
        "installedProviders": PROVIDERS,
        "browserTabs": TABS,
        "recentCommands": RECENT,
    }
    if size in ("xs", "s"):
        snap = {
            "workspace": "winfer",
            "focusedPane": "p2",
            "panes": [{k: p[k] for k in ("id", "provider", "status")} for p in PANES],
        }
    elif size == "large":
        snap = copy.deepcopy(snap)
        snap["browserTabs"] = TABS + [
            {"id": f"t{i}", "title": t, "active": False}
            for i, t in enumerate([
                "llama.cpp server README - GitHub", "Tauri v2 docs - Window customization",
                "React docs - useSyncExternalStore", "Cloudflare dashboard - Workers AI",
                "bridgemind api staging - localhost:8787", "xterm.js addons - fit, webgl, search",
                "Hacker News", "Linear - WIN-142 voice routing",
            ], start=3)
        ]
        snap["recentCommands"] = RECENT + [
            "open localhost 5173", "make the codex one bigger", "tell claude to run the type checker",
            "approve claude", "switch to bridgemind web", "switch to winfer", "equalize panes",
            "open a shell", "close the agy pane", "reload the browser",
        ]
        snap["panes"] = [dict(p, cwd="~/Coding/winfer", branch="main", title=t) for p, t in zip(PANES, [
            "refactoring the pane manager store", "adding voice transcript tests",
            "pnpm tauri dev", "reviewing PR #42 diff",
        ])]
    return snap


def questions(size="full"):
    if size == "xs":
        tools = {k: None for k in TOOLS}
    else:
        tools = dict(TOOLS)
    q = {
        "action_1": {
            "type": "choice",
            "instructions": "The user spoke a voice command to winfer, a terminal app for coding agents. Which app command does the transcript ask for first? The transcript may contain speech-recognition errors.",
            "criteria": tools,
        },
        "action_2": {
            "type": "choice",
            "instructions": "If the transcript asks for a second, different command after the first one, which is it? Otherwise none.",
            # descriptions only on action_1 to save tokens (clef reads all questions jointly)
            "criteria": {k: None for k in TOOLS} if size != "large" else dict(TOOLS),
        },
    }
    if size in ("xs", "s"):
        del q["action_2"]
    for p in (PROVIDERS if size not in ("xs", "s") else []):
        q[f"spawn_panes.count.{p['id']}"] = {
            "type": "choice",
            "instructions": f"How many new {p['id']} panes ({p['name']}) should be opened?",
            "criteria": {str(n): None for n in range(5)},
        }
    if size == "xs":
        return q
    pane_opts = {p["id"]: f"{p['label']} ({p['provider']}, {p['status']})" for p in PANES}
    pane_opts["none"] = "no existing pane is targeted"
    q["target_pane"] = {
        "type": "choice",
        "instructions": "Which existing pane does the command refer to? 'it' or 'this' means the focused pane.",
        "criteria": pane_opts,
    }
    if size == "s":
        return q
    q["resize_amount"] = {
        "type": "score",
        "instructions": "How much should the pane be resized?",
        "criteria": ["slightly", "moderately", "a lot"],
    }
    q["spawn_panes.worktree"] = {
        "type": "noul",
        "instructions": "Did the user ask for the new panes to use separate git worktrees?",
    }
    q["workspace"] = {
        "type": "choice",
        "instructions": "Which workspace does the user name, if any?",
        "criteria": {**{w: None for w in WORKSPACES}, "none": "no workspace named"},
    }
    return q


# ---------------------------------------------------------------------------
# compact (~1k tok) variant for M0.5 task 2: short tool descriptions, only the
# installed providers that matter, tiny string snapshot, sharper action_2/count wording.
# ---------------------------------------------------------------------------
TOOLS_SHORT = {
    "spawn_panes": "Open NEW panes running an agent: claude, codex, agy, grok or a shell.",
    "close_pane": "Close an existing pane and kill what runs in it.",
    "focus_pane": "Move focus to an existing pane: go to, jump to, select.",
    "zoom_pane": "Maximize one pane to fill the window, or un-maximize it.",
    "resize_pane": "Make one existing pane bigger or smaller.",
    "swap_panes": "Swap the positions of two panes.",
    "equalize_panes": "Make all panes the same size.",
    "set_layout": "Change the pane arrangement: grid, columns, rows or stack.",
    "send_to_pane": "Tell or ask one agent to do a task.",
    "broadcast": "Send the same message to every agent at once.",
    "approve_agent": "Approve or accept an agent's pending permission request.",
    "interrupt_agent": "Stop or cancel an agent's current work but keep its pane.",
    "open_url": "Open a website, URL or localhost port in the browser.",
    "browser_new_tab": "Open a new empty browser tab.",
    "browser_close_tab": "Close the current browser tab.",
    "browser_switch_tab": "Switch to another already open browser tab.",
    "browser_back": "Browser: go back one page.",
    "browser_forward": "Browser: go forward one page.",
    "browser_reload": "Reload or refresh the browser page.",
    "set_device": "Change the microphone or audio device.",
    "switch_workspace": "Switch to another existing workspace or project.",
    "create_workspace": "Create a brand new workspace.",
    "toggle_sidebar": "Show or hide the sidebar.",
    "toggle_browser": "Show or hide the browser panel.",
    "load_recipe": "Load a saved recipe: a preset of panes and layout.",
    "show_shortcuts": "Show the keyboard shortcuts and voice commands.",
    "repeat_last": "Repeat the previous voice command.",
    "undo_last": "Undo the previous voice command.",
    "none": "Not an app command: chatter, noise or unrelated speech.",
}

# speech-recognition aliases folded into the count question (cases 3, 4, 8, 10)
PROVIDER_HINT = {
    "claude": "claude / Claude Code; ASR may write cloud, clod, clawed",
    "codex": "codex; ASR may write codecs, code x",
    "agy": "agy / Antigravity; ASR may write gravity, aggie",
    "grok": "grok; ASR may write grock, groc",
    "shell": "plain shell / terminal / zsh",
}


def compact_state(transcript, transcript_first=True):
    panes = "; ".join(f"{p['id']} {p['provider']} {p['status']}" for p in PANES)
    t = f"Transcript: {transcript}"
    s = f"Focused pane: p2. Panes: {panes}."
    return f"{t}\n{s}" if transcript_first else f"{s}\n{t}"


def compact_questions(providers=None):
    providers = providers or [p["id"] for p in PROVIDERS]
    q = {
        "action_1": {
            "type": "choice",
            "instructions": "Voice command to winfer, a terminal app for coding agents; may contain speech-recognition errors. Which command does it ask for first?",
            "criteria": dict(TOOLS_SHORT),
        },
        "action_2": {
            "type": "choice",
            "instructions": "A second, DIFFERENT command explicitly requested after the first one, e.g. 'close X and open Y'. none if the user asked for only one thing; opening several panes at once is ONE command.",
            "criteria": {k: None for k in TOOLS_SHORT},
        },
    }
    for p in providers:
        q[f"spawn_panes.count.{p}"] = {
            "type": "choice",
            "instructions": f"How many NEW {PROVIDER_HINT[p]} panes are requested? Each agent named once counts 1; 0 if not named.",
            "criteria": {str(n): None for n in range(5)},
        }
    pane_opts = {p["id"]: f"{p['provider']} pane, {p['status']}" for p in PANES}
    pane_opts["p2"] += ", focused"
    pane_opts["none"] = "no existing pane"
    q["target_pane"] = {
        "type": "choice",
        "instructions": "Which existing pane does the command act on? 'it' or 'this' means the focused pane.",
        "criteria": pane_opts,
    }
    q["resize_amount"] = {
        "type": "score",
        "instructions": "How much should the pane be resized?",
        "criteria": ["slightly", "moderately", "a lot"],
    }
    q["spawn_panes.worktree"] = {
        "type": "noul",
        "instructions": "Did the user ask for separate git worktrees?",
    }
    q["workspace"] = {
        "type": "choice",
        "instructions": "Which workspace does the user name, if any?",
        "criteria": {**{w: None for w in WORKSPACES}, "none": None},
    }
    return q


def build(transcript, size="full", transcript_first=True, state_as_string=False):
    """transcript_first only matters with state_as_string (object keys are sorted by the server)."""
    if size == "s_rich":  # ~1.3k: full app snapshot + original action_1 descriptions + target_pane only
        return {"model": "clef-flash", "state": {"transcript": transcript, "app": snapshot("full")}, "questions": questions("s")}
    if size == "compact":
        return {"model": "clef-flash", "state": compact_state(transcript, transcript_first), "questions": compact_questions()}
    snap = snapshot(size)
    if state_as_string:
        import json
        t = f"Transcript: {transcript}"
        s = "App snapshot: " + json.dumps(snap, separators=(",", ":"))
        state = f"{t}\n{s}" if transcript_first else f"{s}\n{t}"
    else:
        state = {"transcript": transcript, "app": snap}
    return {"model": "clef-flash", "state": state, "questions": questions(size)}


# ---------------------------------------------------------------------------
# two-stage ~1k design: stage 1 = action_1 + multi (noul) + target_pane (~1k tok);
# stage 2 only asks the arguments needed by the chosen action(s).
# ---------------------------------------------------------------------------
# descriptions only where the key alone is ambiguous (each option costs ~14 tok of template scaffolding anyway)
TOOLS_1K = {k: None for k in TOOLS_SHORT}
TOOLS_1K.update({
    "spawn_panes": "Open NEW panes running claude, codex, agy, grok or a shell; ASR may say cloud, codecs, gravity.",
    "close_pane": "Close a pane and kill what runs in it.",
    "focus_pane": "Move focus to an existing pane.",
    "zoom_pane": "Maximize or un-maximize one pane.",
    "send_to_pane": "Tell or ask one agent to do a task.",
    "broadcast": "Send one message to every agent.",
    "approve_agent": "Accept an agent's pending permission request.",
    "interrupt_agent": "Stop an agent's current work but keep its pane.",
    "open_url": "Open a website, URL or localhost port.",
    "set_device": "Change microphone or audio device.",
    "load_recipe": "Load a saved preset of panes and layout.",
    "none": "Not an app command: chatter, noise or unrelated speech.",
})

MULTI_Q = {
    "type": "noul",
    "instructions": "Does the user explicitly ask for a second, DIFFERENT command after the first one, e.g. 'close X and open Y'? No if they asked for only one thing; opening several panes at once is ONE command.",
}


def stage1(transcript):
    q = compact_questions()
    return {"model": "clef-flash", "state": compact_state(transcript), "questions": {
        "action_1": {**q["action_1"], "criteria": dict(TOOLS_1K)},
        "multi": dict(MULTI_Q),
        "target_pane": q["target_pane"],
    }}


def stage2(transcript, a1, multi):
    """returns None if nothing else to ask"""
    q = compact_questions()
    out = {}
    if multi:
        out["action_2"] = {"type": "choice",
                           "instructions": f"The first command is {a1}. Which second, DIFFERENT command does the user ask for after it?",
                           "criteria": {k: None for k in TOOLS_SHORT if k not in (a1, "none")}}
    want = {a1} | ({"spawn_panes"} if multi else set())  # spawn args are needed if spawn may be the 2nd action
    if "spawn_panes" in want:
        out.update({k: v for k, v in q.items() if k.startswith("spawn_panes.")})
    if a1 == "resize_pane":
        out["resize_amount"] = q["resize_amount"]
    if a1 == "switch_workspace" or multi:
        out["workspace"] = q["workspace"]
    if not out:
        return None
    return {"model": "clef-flash", "state": compact_state(transcript), "questions": out}
