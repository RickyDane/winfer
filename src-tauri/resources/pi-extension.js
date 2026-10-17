// winfer's pi extension (see src-tauri/src/hooks.rs), loaded with `--extension`: registers the
// MCP servers winfer passes in `WINFER_PI_MCP` and forwards the events that change a pane's
// status (run started / settled, blocking prompts) and its context use to winfer's hook server.
const { WINFER_HOOK: hook, WINFER_TOKEN: token, WINFER_PANE_ID: pane, WINFER_PI_MCP: mcp } = process.env;

const send = (event, body) =>
  fetch(`http://${hook}/hook/pi/${pane}/${event}`, {
    method: "POST",
    headers: { "X-Winfer-Token": token ?? "" },
    body: JSON.stringify(body),
  }).catch(() => {});

/** The text of the run's last assistant message. */
function lastReply(messages = []) {
  for (const m of [...messages].reverse()) {
    if (m?.role !== "assistant" || !Array.isArray(m.content)) continue;
    const text = m.content.filter((c) => c?.type === "text").map((c) => c.text).join("\n").trim();
    if (text) return text;
  }
}

export default function (pi) {
  let servers = {};
  try {
    servers = JSON.parse(mcp ?? "{}");
  } catch {}
  // Declared directly, so the model sees winfer's tools without searching for them first.
  for (const [name, config] of Object.entries(servers)) {
    try {
      pi.registerMcpServer(name, { exposure: "direct", ...config });
    } catch {}
  }

  if (!hook || !pane) return;
  const session = (ctx) => {
    try {
      return ctx.sessionManager.getSessionId();
    } catch {}
  };
  // How full the context window is, for the pane header's meter (unknown right after compaction).
  const reportContext = (ctx) => {
    let usage;
    try {
      usage = ctx.getContextUsage();
    } catch {}
    if (usage?.tokens == null || !usage.contextWindow) return;
    void send("usage", { context: { usedPercent: usage.percent, tokens: usage.tokens, window: usage.contextWindow, at: Date.now() } });
  };
  let reply;
  pi.on("session_start", (_event, ctx) => {
    void send("session_start", { session_id: session(ctx) });
    reportContext(ctx);
  });
  pi.on("turn_end", (_event, ctx) => reportContext(ctx));
  pi.on("session_compact", (_event, ctx) => reportContext(ctx));
  pi.on("model_select", (_event, ctx) => reportContext(ctx));
  pi.on("agent_start", (_event, ctx) => {
    reply = undefined;
    void send("agent_start", { session_id: session(ctx) });
  });
  pi.on("agent_end", (event) => {
    reply = lastReply(event.messages) ?? reply;
  });
  pi.on("agent_settled", (event, ctx) =>
    void send(event.aborted ? "agent_aborted" : "agent_settled", { session_id: session(ctx), last_assistant_message: reply }),
  );
  pi.on("ui_prompt_start", (event, ctx) => void send("ui_prompt_start", { session_id: session(ctx), message: event.title }));
  pi.on("ui_prompt_end", (_event, ctx) => void send("ui_prompt_end", { session_id: session(ctx) }));
}
