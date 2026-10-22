/**
 * Questions agents ask the user through winfer's `ask_user` MCP tool (for agents without a
 * question tool of their own, like Codex). They show as an in-app dialog and, on the paired
 * phones, as that pane's question, so either side can answer; the tool call waits until then.
 */
import { create } from "zustand";
import type { AgentRequest } from "@/features/status/request";

export interface AskOption {
  label: string;
  description?: string;
}

export interface Ask {
  header?: string;
  question: string;
  options: AskOption[];
  multiSelect: boolean;
}

export interface PendingQuestions {
  id: number;
  paneId: string;
  questions: Ask[];
  /** Answers to the questions before the current one. */
  answers: string[];
  askedAt: number;
  /** The answers, one per question; null when the user dismissed them. */
  resolve: (answers: string[] | null) => void;
}

export const useQuestions = create<{ queue: PendingQuestions[] }>(() => ({ queue: [] }));

let serial = 0;

const str = (v: unknown) => (typeof v === "string" ? v.trim() : "");

/** `ask_user`'s `questions` argument, with empty questions and options dropped. */
export function parseQuestions(raw: unknown): Ask[] {
  if (!Array.isArray(raw)) return [];
  return raw.flatMap((q): Ask[] => {
    const question = str(q?.question);
    if (!question) return [];
    const options = (Array.isArray(q.options) ? q.options : [])
      .map((o: unknown): AskOption => (typeof o === "string" ? { label: o.trim() } : { label: str((o as AskOption)?.label), description: str((o as AskOption)?.description) || undefined }))
      .filter((o: AskOption) => o.label);
    return [{ header: str(q.header) || undefined, question, options, multiSelect: q.multiSelect === true }];
  });
}

export function askQuestions(paneId: string, questions: Ask[]): Promise<string[] | null> {
  return new Promise((resolve) => {
    const pending: PendingQuestions = { id: ++serial, paneId, questions, answers: [], askedAt: Date.now(), resolve };
    useQuestions.setState((s) => ({ queue: [...s.queue, pending] }));
  });
}

/** Answers the current question of `id`; the last answer resolves the call. False if it's gone. */
export function answerQuestion(id: number, answer: string): boolean {
  const pending = useQuestions.getState().queue.find((q) => q.id === id);
  const text = answer.trim();
  if (!pending || !text) return false;
  const answers = [...pending.answers, text];
  if (answers.length < pending.questions.length) {
    useQuestions.setState((s) => ({ queue: s.queue.map((q) => (q === pending ? { ...q, answers } : q)) }));
  } else {
    useQuestions.setState((s) => ({ queue: s.queue.filter((q) => q !== pending) }));
    pending.resolve(answers);
  }
  return true;
}

/** Dismisses all of `id`'s questions unanswered. */
export function dismissQuestions(id: number): boolean {
  const pending = useQuestions.getState().queue.find((q) => q.id === id);
  if (!pending) return false;
  useQuestions.setState((s) => ({ queue: s.queue.filter((q) => q !== pending) }));
  pending.resolve(null);
  return true;
}

/** Drops a pane's questions (it closed or its call gave up). */
export function dropQuestions(paneId: string) {
  for (const q of useQuestions.getState().queue.filter((q) => q.paneId === paneId)) dismissQuestions(q.id);
}

/** The oldest questions an agent in `paneId` is waiting on. */
export const questionsFor = (paneId: string) => useQuestions.getState().queue.find((q) => q.paneId === paneId);

export const currentQuestion = (p: PendingQuestions) => p.questions[p.answers.length];

/**
 * The current question as a phone request: options numbered from 1, then a "Type something"
 * row for an own answer.
 */
export function questionRequest(p: PendingQuestions): AgentRequest {
  const q = currentQuestion(p);
  const progress = p.questions.length > 1 ? ` (${p.answers.length + 1}/${p.questions.length})` : "";
  const head = q.header ? `${q.header}${progress}\n` : progress ? `${progress.trim()}\n` : "";
  const multi = q.multiSelect ? "\n(Several apply? Type them, comma-separated.)" : "";
  const options = q.options.map((o, i) => ({ n: i + 1, label: o.label, detail: o.description }));
  return { kind: "question", text: `${head}${q.question}${multi}`, options: [...options, { n: options.length + 1, label: "Type something" }] };
}

/** The tool result for the user's answers. */
export const answersText = (questions: Ask[], answers: string[]) =>
  `The user answered:\n${questions.map((q, i) => `"${q.question}" → "${answers[i]}"`).join("\n")}`;

export const DISMISSED = "The user dismissed the questions without answering. Don't ask again; go on with your best judgement or ask in plain text.";
