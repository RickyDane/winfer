/**
 * Runs the requests the ChatGPT-plan voice model hands over (it can't call tools itself): a
 * tool-calling model on the user's ChatGPT plan (src-tauri/src/chatgpt_voice.rs) works through
 * winfer's tools and answers in a sentence or two for the voice to speak.
 */
import { invoke } from "@tauri-apps/api/core";
import { toolContext } from "@/features/palette/entries";
import { DELEGATE_ROLE, stateSummary } from "./instructions";
import { END_CONVERSATION, runVoiceFunction, voiceTools } from "./tools";

type Item = Record<string, unknown>;

/** Model turns per request; a request needing more is answered with what was done. */
const MAX_STEPS = 10;
/** Items kept between requests, so follow-ups ("yes", "the other one") have context. */
const KEEP_ITEMS = 60;

export interface DelegateHooks {
  /** A tool starts (name) or none is running (null). */
  onTool: (name: string | null) => void;
  /** A tool finished, with its result. */
  onResult: (name: string, args: Record<string, unknown>, output: string) => void;
  /** The tool list changed (browser tools loaded). */
  onToolsChanged?: () => void;
}

export interface DelegateAnswer {
  text: string;
  /** The user asked to end the conversation. */
  end: boolean;
}

/** The answer text of a model turn's message items. */
export function answerText(items: Item[]): string {
  return items
    .filter((i) => i.type === "message")
    .flatMap((i) => (i.content as { type: string; text?: string }[] | undefined) ?? [])
    .filter((c) => c.type === "output_text" && c.text)
    .map((c) => c.text)
    .join(" ")
    .trim();
}

/** Drops whole requests from the front, so no tool output loses its call. */
export function trimHistory(items: Item[], keep = KEEP_ITEMS): Item[] {
  if (items.length <= keep) return items;
  const starts = items.flatMap((i, n) => (i.role === "user" ? [n] : []));
  const cut = starts.find((n) => items.length - n <= keep) ?? starts.at(-1) ?? 0;
  return items.slice(cut);
}

export class Delegate {
  private history: Item[] = [];
  constructor(private hooks: DelegateHooks) {}

  async run(request: string): Promise<DelegateAnswer> {
    this.history = trimHistory([...this.history, { role: "user", content: [{ type: "input_text", text: request }] }]);
    let end = false;
    for (let step = 0; step < MAX_STEPS; step++) {
      const output = await invoke<Item[]>("chatgpt_tool_turn", {
        model: "",
        instructions: `${DELEGATE_ROLE}\n\n${stateSummary(toolContext())}`,
        input: [...this.history],
        tools: (await voiceTools("full")).map(({ type, name, description, parameters }) => ({ type, name, description, parameters })),
      });
      this.history.push(...output);
      const calls = output.filter((i) => i.type === "function_call") as { call_id: string; name: string; arguments?: string }[];
      if (calls.length === 0) return { text: answerText(output) || "Done.", end };
      for (const call of calls) {
        let args: Record<string, unknown> = {};
        try {
          args = call.arguments ? JSON.parse(call.arguments) : {};
        } catch {
          /* the tool reports missing arguments */
        }
        let result: string;
        if (call.name === END_CONVERSATION) {
          end = true;
          result = "The conversation ends after your answer.";
        } else {
          this.hooks.onTool(call.name);
          result = await runVoiceFunction(call.name, args, () => this.hooks.onToolsChanged?.());
          this.hooks.onTool(null);
          this.hooks.onResult(call.name, args, result);
        }
        this.history.push({ type: "function_call_output", call_id: call.call_id, output: result });
      }
    }
    return { text: "That took more steps than I can take at once; tell me if I should go on.", end };
  }
}

/** Splits an answer for `delegation.context.append` (at most `max` bytes each, on char boundaries). */
export function chunks(text: string, max = 500): string[] {
  const out: string[] = [];
  const enc = new TextEncoder();
  let current = "";
  for (const ch of text) {
    if (enc.encode(current + ch).length > max) {
      out.push(current);
      current = "";
    }
    current += ch;
  }
  out.push(current);
  return out;
}
