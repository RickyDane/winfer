/**
 * Voice → Clef (System One) → tools.
 *
 * The request asks Clef to pick tools from the Tool Registry (their descriptions are the option
 * text) and to fill every parameter as a typed question. Clef can only choose among the options we
 * give it, so it can't invent commands. Free text (URLs, prompts) is picked from spans of the
 * transcript. No grammar parsing happens anywhere.
 */
import type { Command } from "@/commands/bus";
import { TOOLS, type Args, type ParamDef, type ToolContext, type ToolDef } from "@/tools/registry";
import { leaves } from "@/features/layout/tree";
import { providerName } from "@/stores/workspaces";

const NUMBER_WORDS = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine"];
const MAX_CHOICES = 255;

/**
 * Gate on how clearly the chosen tool beats the runner-up (top probability ÷ second). Clef's own
 * `confidence` doesn't separate right from wrong on the Workers AI model; this ratio does
 * (voice-evals, Oct 2026: correct median ≈ 8, wrong answers ≤ 1.12 apart from argument mistakes).
 */
export const AUTO_RUN = 2;
export const CONFIRM = 1.05;

type Question =
  | { type: "choice"; instructions: string; criteria: Record<string, string | null> }
  | { type: "noul"; instructions: string }
  | { type: "score"; instructions: string; criteria: string[] };

export interface DecisionRequest {
  state: unknown;
  questions: Record<string, Question>;
}

type Answer =
  | { type: "choice"; choice: string; probabilities?: Record<string, number>; confidence?: number }
  | { type: "noul"; noul: number }
  | { type: "score"; score: number; probabilities?: Record<string, number>; confidence?: number };

export interface DecisionResult {
  answers: Record<string, Answer>;
}

/** Pane targets are shared by many tools, so they're asked once. */
const SHARED_PARAMS = new Set(["target", "model"]);
/** Free-text parameters share one question per slot (an agent instruction, a web address). */
/** Yes/no "is there any?" per slot: choice questions almost never pick "none" over a span. */
const SLOT_PRESENT = {
  instruction: "Did the user give an instruction or message for an agent (something for it to do or type, like “create a Flappy Bird clone”), beyond just opening, closing or arranging panes?",
  address: "Did the user mention a website, web address or localhost port to open?",
} as const;

const SLOT_QUESTIONS = {
  instruction:
    "What instruction or message should go to the agent(s)? Only the instruction itself, e.g. “create a Flappy Bird clone”, " +
    "without lead-ins like “and tell it to” or “ask them to”. Answer none if the user gave no instruction.",
  address: "Which words are the website, web address or localhost port to open? Answer none if no address was mentioned.",
} as const;
const MAX_SHORT_SPAN = 6;

function questionKey(tool: ToolDef, param: string, option?: string) {
  if (SHARED_PARAMS.has(param)) return param;
  const def = tool.params[param];
  if (def?.kind === "span") return def.slot;
  return [tool.id, param, option].filter(Boolean).join(".");
}

/**
 * Candidate stretches of the transcript for free text: short phrases (names, URLs) plus every
 * ending of the sentence (instructions usually come last: "… and tell it to <instruction>").
 */
export function transcriptSpans(transcript: string): string[] {
  const words = transcript.replace(/[.,!?;]+(\s|$)/g, "$1").split(/\s+/).filter(Boolean);
  const spans = new Set<string>();
  for (let start = 0; start < words.length; start++) {
    for (let len = 1; start + len <= words.length; len++) {
      if (len <= MAX_SHORT_SPAN || start + len === words.length) spans.add(words.slice(start, start + len).join(" "));
    }
  }
  // Shortest first: when unsure the model leans toward earlier options, and a short
  // phrase is a safer default than the whole sentence.
  return [...spans].sort((a, b) => a.length - b.length).slice(0, MAX_CHOICES - 1);
}

function question(param: ParamDef, ctx: ToolContext, transcript: string, prefix: string): Question | Question[] | null {
  const instructions = `${prefix}${param.ask}`;
  switch (param.kind) {
    case "enum": {
      const options = param.options(ctx);
      return Object.keys(options).length >= 2 ? { type: "choice", instructions, criteria: options } : null;
    }
    case "count":
      return { type: "choice", instructions, criteria: Object.fromEntries(NUMBER_WORDS.slice(0, param.max + 1).map((w) => [w, null])) };
    case "bool":
      return { type: "noul", instructions };
    case "level":
      return { type: "score", instructions, criteria: param.levels };
    case "span": {
      const spans = transcriptSpans(transcript);
      return spans.length
        ? {
            type: "choice",
            instructions: SLOT_QUESTIONS[param.slot],
            criteria: Object.fromEntries([["none", "Not mentioned"], ...spans.map((s) => [s, null])]),
          }
        : null;
    }
    case "perOption":
      return null; // expanded by the caller
  }
}

export function buildRequest(transcript: string, ctx: ToolContext, recent: string[] = []): DecisionRequest {
  const panes = ctx.active.layout ? leaves(ctx.active.layout) : [];
  const state = {
    transcript,
    app: {
      workspace: ctx.active.name,
      focusedPane: ctx.focusedPaneId ? panes.indexOf(ctx.focusedPaneId) + 1 : null,
      panes: panes.map((id, i) => {
        const pane = ctx.active.panes.find((p) => p.id === id)!;
        return { number: i + 1, provider: providerName(ctx.providers, pane.provider), status: pane.status, branch: pane.worktree?.branch ?? null };
      }),
      workspaces: ctx.workspaces.map((w) => w.name),
      installedProviders: ctx.providers.filter((p) => p.installed).map((p) => p.name),
      browserTabs: ctx.browserTabs,
      recentCommands: recent,
    },
  };

  const tools = Object.fromEntries(TOOLS.map((t) => [t.id, t.description]));
  // The second action only needs short labels; full descriptions are already in action_1.
  const titles = Object.fromEntries(TOOLS.map((t) => [t.id, t.title]));
  const questions: Record<string, Question> = {
    action_1: { type: "choice", instructions: "Which tool does the user's first requested action map to?", criteria: tools },
    action_2: {
      type: "choice",
      instructions: "Which tool does a second, different action map to? Answer none unless the user clearly asked for two different things.",
      criteria: titles,
    },
  };

  for (const tool of TOOLS) {
    for (const [name, param] of Object.entries(tool.params)) {
      const key = questionKey(tool, name);
      if (questions[key]) continue;
      const prefix = SHARED_PARAMS.has(name) || param.kind === "span" ? "" : `If the user wants “${tool.title}”: `;
      if (param.kind === "perOption") {
        for (const [option, label] of Object.entries(param.options(ctx))) {
          questions[questionKey(tool, name, option)] = {
            type: "choice",
            instructions: `${prefix}${param.ask} (${label})`,
            criteria: Object.fromEntries(NUMBER_WORDS.slice(0, param.max + 1).map((w) => [w, null])),
          };
        }
        continue;
      }
      const q = question(param, ctx, transcript, prefix);
      if (q && !Array.isArray(q)) questions[key] = q;
      if (q && param.kind === "span") questions[`${key}_given`] = { type: "noul", instructions: SLOT_PRESENT[param.slot] };
    }
  }
  return { state, questions };
}

export interface Decision {
  tool: ToolDef;
  args: Args;
  /** Top probability divided by the runner-up's; higher means a clearer choice. */
  confidence: number;
  /** Next-best tools for the confirmation prompt. */
  alternatives: { tool: ToolDef; probability: number }[];
}

const byId = (id: string) => TOOLS.find((t) => t.id === id);

function argsFor(tool: ToolDef, answers: Record<string, Answer>, ctx: ToolContext): Args {
  const args: Args = {};
  for (const [name, param] of Object.entries(tool.params)) {
    if (param.kind === "perOption") {
      args[name] = Object.fromEntries(
        Object.keys(param.options(ctx)).map((option) => {
          const a = answers[questionKey(tool, name, option)];
          return [option, a?.type === "choice" ? Math.max(0, NUMBER_WORDS.indexOf(a.choice)) : 0];
        }),
      );
      continue;
    }
    const answer = answers[questionKey(tool, name)];
    if (!answer) {
      // Skipped single-option enums resolve to their only option.
      if (param.kind === "enum") args[name] = Object.keys(param.options(ctx))[0];
      continue;
    }
    if (answer.type === "noul") args[name] = answer.noul >= 0.5;
    else if (answer.type === "score" && param.kind === "level") {
      const best = Object.entries(answer.probabilities ?? {}).sort((a, b) => b[1] - a[1])[0]?.[0];
      args[name] = best ?? param.levels[Math.round(Math.min(Math.max(answer.score, 0), param.levels.length - 1))];
    } else if (answer.type === "choice") {
      if (param.kind === "count") args[name] = Math.max(0, NUMBER_WORDS.indexOf(answer.choice));
      else if (param.kind === "span") {
        const given = answers[`${param.slot}_given`];
        const present = given?.type === "noul" ? given.noul >= 0.5 : true;
        args[name] = !present || answer.choice === "none" ? undefined : param.slot === "instruction" ? withoutLeadIn(answer.choice) : answer.choice;
      }
      else args[name] = answer.choice;
    }
  }
  return args;
}

/**
 * Clef sometimes picks the instruction together with how it was addressed ("ask it to add
 * dark mode", "Claude to run the tests"). Trims that framing from the extracted text only;
 * which tool runs is still decided entirely by Clef.
 */
export function withoutLeadIn(text: string): string {
  const trimmed = text
    .replace(/^(and|then|please)\s+/i, "")
    .replace(/^(?:(?:tell|ask|have|get|let)\s+)?(?:it|them|him|her|both|all|each|everyone|[\w-]+(?:\s+(?:one|code|agent|agents))?)\s+to\s+/i, "")
    // German: "(sag Claude,) er soll …", "sie sollen …"
    .replace(/^(?:(?:sag|sage|bitte)\s+)?(?:(?:ihm|ihr|ihnen|[\w-]+),?\s+)?(?:er|sie|es)\s+soll(?:en)?\s+/i, "")
    .replace(/^(insert|type|send|say)\s+/i, "")
    .replace(/\s+into\s+(?:pane\s+\w+|[\w-]+(?:'s)?(?:\s+(?:pane|prompt|window))?)$/i, "");
  return trimmed || text;
}

/** Turns Clef's answers into up to two decisions (the second only if it's a different tool). */
export function parseDecision(result: DecisionResult, ctx: ToolContext): Decision[] {
  const decisions: Decision[] = [];
  for (const key of ["action_1", "action_2"]) {
    const answer = result.answers[key];
    if (answer?.type !== "choice") continue;
    const tool = byId(answer.choice);
    if (!tool || tool.id === "none") continue;
    if (key === "action_2" && decisions.some((d) => d.tool.id === tool.id)) continue;
    if (key === "action_2" && decisions.length === 0) continue;
    const alternatives = Object.entries(answer.probabilities ?? {})
      .filter(([id]) => id !== tool.id && id !== "none")
      .sort((a, b) => b[1] - a[1])
      .slice(0, 2)
      .flatMap(([id, probability]) => (byId(id) ? [{ tool: byId(id)!, probability }] : []));
    decisions.push({ tool, args: argsFor(tool, result.answers, ctx), confidence: clarity(answer.probabilities, tool.id), alternatives });
  }
  // "Open Codex and ask it to …": the new agent already gets the instruction as its prompt,
  // so a second "tell an agent" would type it into some other pane.
  if (decisions.some((d) => d.tool.id === "spawn_panes" && d.args.prompt)) {
    return decisions.filter((d) => d.tool.id !== "send_to_pane");
  }
  return decisions;
}

/** Ratio of the chosen option's probability to the best other option's. */
export function clarity(probabilities: Record<string, number> | undefined, chosen: string): number {
  if (!probabilities) return 0;
  const top = probabilities[chosen] ?? 0;
  const second = Math.max(0, ...Object.entries(probabilities).filter(([id]) => id !== chosen).map(([, p]) => p));
  return second > 0 ? top / second : top > 0 ? Infinity : 0;
}

export type Verdict = "run" | "confirm" | "reject";

export function verdict(decisions: Decision[]): Verdict {
  if (decisions.length === 0) return "reject";
  const lowest = Math.min(...decisions.map((d) => d.confidence));
  if (lowest < CONFIRM) return "reject";
  if (lowest < AUTO_RUN || decisions.some((d) => d.tool.risk === "confirm")) return "confirm";
  return "run";
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
