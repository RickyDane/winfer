/**
 * Dev-only: `?codeweb` opens a workspace of winfer code panes shown as the web UI (see
 * features/code), each driven by a scripted session.
 */
export { CODE_WEB_DEMO } from "@/features/code/styles";

const PROMPTS = ["Add exponential backoff to retry() and make it cancellable"];

export async function seedCodeWeb() {
  const w = window as { __codeWebSeeded?: boolean };
  if (w.__codeWebSeeded) return;
  w.__codeWebSeeded = true;
  const { useWorkspaces } = await import("@/stores/workspaces");
  while (!useWorkspaces.getState().hydrated) await new Promise((r) => setTimeout(r, 50));
  // Earlier previews' workspaces go, so the demo always starts fresh.
  for (const ws of useWorkspaces.getState().workspaces) if (ws.name.startsWith("winfer code ·")) useWorkspaces.getState().removeWorkspace(ws.id);
  const workspaceId = useWorkspaces.getState().createWorkspace("winfer code · web", "/Users/you/winfer");
  useWorkspaces.getState().setActiveWorkspace(workspaceId);
  let prev: string | undefined;
  for (const prompt of PROMPTS) prev = useWorkspaces.getState().addPane("winfer", { workspaceId, prompt, splitFrom: prev });
}
