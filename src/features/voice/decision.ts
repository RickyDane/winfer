/**
 * A voice command's tool calls, as planned by the LLM (planner.ts): which tool of the Tool
 * Registry to run, with which arguments.
 */
import type { Command } from "@/commands/bus";
import type { Args, ToolContext, ToolDef } from "@/tools/registry";

export interface Decision {
  tool: ToolDef;
  args: Args;
}

/** Spawn order follows the order providers were mentioned ("claude, codex and grok"). */
export function orderByMention(commands: Command[], transcript: string, ctx: ToolContext): Command[] {
  const text = transcript.toLowerCase();
  const position = (provider: string) => {
    const p = ctx.providers.find((x) => x.id === provider);
    const names = [p?.name, p?.id, ...(p?.aliases ?? [])].filter(Boolean).map((n) => n!.toLowerCase());
    const hits = names.map((n) => text.indexOf(n)).filter((i) => i >= 0);
    return hits.length ? Math.min(...hits) : Infinity;
  };
  return [...commands].sort((a, b) =>
    a.type === "pane.spawn" && b.type === "pane.spawn" ? position(a.provider) - position(b.provider) : 0,
  );
}
