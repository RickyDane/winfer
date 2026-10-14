// winfer imports this file as an ES module and calls activate(api) when the plugin is turned on.
export function activate(api) {
  api.registerTool({
    id: "summarize_panes",
    title: "Count agents by status",
    description: "Say how many agents are working, waiting for input or done",
    palette: true,
    run() {
      const { workspaces } = api.state();
      const counts = {};
      for (const pane of workspaces.flatMap((w) => w.panes)) counts[pane.status] = (counts[pane.status] ?? 0) + 1;
      api.toast(Object.entries(counts).map(([s, n]) => `${n} ${s}`).join(", ") || "No agents running");
    },
  });

  // Anything returned from activate (or from api.on / registerTool / registerCommand) is cleaned up on disable.
  return api.on("pane.status", (event) => {
    if (event.status === "done") api.toast(`${event.provider} finished`, "success");
  });
}
