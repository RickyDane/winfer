/** The in-app dialog for agents' `ask_user` questions (see questions.ts). */
import { MessageCircleQuestion } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { providerName, useWorkspaces } from "@/stores/workspaces";
import { answerQuestion, currentQuestion, dismissQuestions, useQuestions } from "./questions";

export function QuestionDialog() {
  const pending = useQuestions((s) => s.queue[0]);
  const waiting = useQuestions((s) => s.queue.length - 1);
  const asker = useWorkspaces((s) => {
    const pane = pending && s.workspaces.flatMap((w) => w.panes).find((p) => p.id === pending.paneId);
    if (!pane) return "";
    const ws = s.workspaces.find((w) => w.id === pane.workspaceId);
    return `${providerName(s.providers, pane.provider)}${ws ? ` · ${ws.name}` : ""}`;
  });
  const [picked, setPicked] = useState<number[]>([]);
  const [other, setOther] = useState("");
  const firstRef = useRef<HTMLButtonElement>(null);
  const step = pending ? `${pending.id}:${pending.answers.length}` : "";

  useEffect(() => {
    setPicked([]);
    setOther("");
    if (step) firstRef.current?.focus();
  }, [step]);

  if (!pending) return null;
  const q = currentQuestion(pending);
  const total = pending.questions.length;
  const submit = (answer: string) => answerQuestion(pending.id, answer);
  const submitMulti = () => submit([...picked.sort((a, b) => a - b).map((i) => q.options[i].label), other.trim()].filter(Boolean).join(", "));
  const toggle = (i: number) => setPicked((p) => (p.includes(i) ? p.filter((x) => x !== i) : [...p, i]));

  return (
    <div className="anim-backdrop fixed inset-0 z-[60] flex items-center justify-center bg-black/40">
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="wf-question-title"
        className="anim-dialog flex max-h-[80vh] w-[500px] flex-col rounded-xl border border-border-strong bg-panel-raised shadow-2xl"
      >
        <header className="flex items-start gap-3 border-b border-border px-5 py-3">
          <MessageCircleQuestion size={16} className="mt-0.5 shrink-0 text-accent" />
          <div className="min-w-0 flex-1">
            <h1 id="wf-question-title" className="text-sm font-semibold text-fg">
              {q.header ?? "Question"}
              {total > 1 && <span className="ml-2 text-xs font-normal text-fg-subtle">{`${pending.answers.length + 1}/${total}`}</span>}
            </h1>
            <p className="text-xs text-fg-subtle">{asker} asks · you can also answer from your phone</p>
          </div>
        </header>
        <div className="flex flex-col gap-2 overflow-y-auto px-5 py-4">
          <p className="whitespace-pre-wrap break-words text-[13px] text-fg select-text">{q.question}</p>
          {q.options.map((o, i) => (
            <button
              key={i}
              ref={i === 0 ? firstRef : undefined}
              onClick={() => (q.multiSelect ? toggle(i) : submit(o.label))}
              className={`flex items-start gap-2 rounded-md border px-3 py-2 text-left text-[13px] hover:bg-hover ${picked.includes(i) ? "border-accent bg-hover" : "border-border"}`}
            >
              <span className="w-4 shrink-0 text-fg-subtle">{q.multiSelect ? (picked.includes(i) ? "☑" : "☐") : `${i + 1}.`}</span>
              <span className="min-w-0">
                <span className="text-fg">{o.label}</span>
                {o.description && <span className="block text-xs text-fg-subtle">{o.description}</span>}
              </span>
            </button>
          ))}
          <form
            onSubmit={(e) => {
              e.preventDefault();
              if (q.multiSelect) submitMulti();
              else submit(other);
            }}
          >
            <input
              value={other}
              onChange={(e) => setOther(e.target.value)}
              placeholder={q.multiSelect ? "Something else (optional)…" : "Type your own answer…"}
              className="w-full rounded-md border border-border bg-panel px-3 py-1.5 text-[13px] text-fg outline-none focus:border-accent"
            />
          </form>
        </div>
        <footer className="flex items-center gap-2 border-t border-border px-5 py-3">
          {waiting > 0 && <span className="text-xs text-fg-subtle">{waiting} more waiting</span>}
          <span className="flex-1" />
          <button onClick={() => dismissQuestions(pending.id)} className="rounded-md px-2.5 py-1 text-xs text-fg-muted hover:bg-hover hover:text-fg">
            Dismiss
          </button>
          {(q.multiSelect || other.trim()) && (
            <button
              onClick={() => (q.multiSelect ? submitMulti() : submit(other))}
              disabled={q.multiSelect ? !picked.length && !other.trim() : !other.trim()}
              className="rounded-md bg-accent px-3 py-1 text-xs font-medium text-on-accent hover:opacity-90 disabled:opacity-50"
            >
              {pending.answers.length + 1 < total ? "Next" : "Answer"}
            </button>
          )}
        </footer>
      </div>
    </div>
  );
}
