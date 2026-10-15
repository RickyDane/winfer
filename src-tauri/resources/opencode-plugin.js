// winfer's OpenCode 2 plugin (see src-tauri/src/hooks.rs): forwards the events that change a
// pane's status (turn started / finished, permission prompts, questions) to winfer's hook server.
const { WINFER_HOOK: hook, WINFER_TOKEN: token, WINFER_PANE_ID: pane } = process.env;

const send = (event, sessionID, message) =>
  fetch(`http://${hook}/hook/opencode/${pane}/${event}`, {
    method: "POST",
    headers: { "X-Winfer-Token": token ?? "" },
    body: JSON.stringify({ session_id: sessionID, message }),
  }).catch(() => {});

function message(type, data) {
  if (type === "permission.asked") {
    return `OpenCode needs your permission: ${[data.action, ...(data.resources ?? [])].filter(Boolean).join(" ")}`;
  }
  if (type === "form.created") {
    const field = data.form?.fields?.[0];
    return field?.description || field?.title || data.form?.title;
  }
  if (type === "session.execution.failed") return `OpenCode stopped: ${data.error?.message ?? "an error occurred"}`;
}

// Turn events of subagent sessions are left out: only the main session finishing means done.
// Prompts and questions are forwarded from any session, since they all wait for the user.
const TURN = ["session.execution.started", "session.execution.succeeded", "session.execution.failed", "session.execution.interrupted"];
const PROMPT = ["permission.asked", "permission.replied", "form.created", "form.replied", "form.cancelled"];

export default {
  id: "winfer.status",
  setup(ctx) {
    if (!hook || !pane) return;
    const subagents = new Set();
    const controller = new AbortController();
    void (async () => {
      for await (const { type, data = {} } of ctx.event.subscribe({ signal: controller.signal })) {
        const sessionID = data.sessionID ?? data.form?.sessionID;
        if (type === "session.created" && data.parentID) subagents.add(sessionID);
        else if ((TURN.includes(type) && !subagents.has(sessionID)) || PROMPT.includes(type)) {
          void send(type, sessionID, message(type, data));
        }
      }
    })().catch(() => {});
    return () => controller.abort();
  },
};
