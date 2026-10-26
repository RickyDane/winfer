import { useEffect, useState } from "react";
import {
  Bot,
  Circle,
  FilePen,
  FileText,
  Globe,
  OctagonX,
  Search,
  SquareTerminal,
} from "lucide-react";
import clsx from "clsx";
import type { TaskEv } from "./protocol";
import type { CodeTask } from "./store";
import { modelName } from "./names";

/* Running subagents and background shells as a compact activity timeline, shown in the popover
   of the pane header's activity chip (BgIndicator): one line per shell, and for agents a second line with their recent tool calls. */

export interface BgProps {
  /** Every task of the session; only the running ones are shown. */
  tasks: CodeTask[];
  openLog: (id: string) => void;
  stop: (id: string) => void;
}

function useNow() {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  return now;
}

function clock(ms: number) {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const pad = (n: number) => String(n).padStart(2, "0");
  return h ? `${h}:${pad(m)}:${pad(s % 60)}` : `${m}:${pad(s % 60)}`;
}

const running = <T extends TaskEv,>(ts: T[]) => ts.filter((t) => t.status === "running");

/** A tool icon for a `last` line like "Read src/x.ts". */
function ToolIcon({ line }: { line: string }) {
  const w = line.split(/[\s(]/)[0].toLowerCase();
  if (w === "read") return <FileText size={10} />;
  if (w === "grep" || w === "glob" || w === "search") return <Search size={10} />;
  if (w === "edit" || w === "write" || w === "multiedit") return <FilePen size={10} />;
  if (w === "bash") return <SquareTerminal size={10} />;
  if (w.startsWith("web")) return <Globe size={10} />;
  return <Circle size={6} />;
}

/** Stop, slid in on hover; it stays while the task is stopping. */
function StopButton({ t, stop }: { t: TaskEv; stop: (id: string) => void }) {
  const [stopping, setStopping] = useState(false);
  return (
    <div className="cw-bg-actions">
      <button
        className={clsx("cw-stop-btn", stopping && "busy")}
        title={stopping ? "Stopping…" : "Stop this task"}
        disabled={stopping}
        onClick={(e) => {
          e.stopPropagation();
          setStopping(true);
          stop(t.id);
        }}
      >
        {stopping ? <span className="cw-spinner" /> : <OctagonX size={13} />}
        <span className="cw-stop-label">{stopping ? "Stopping…" : "Stop"}</span>
      </button>
    </div>
  );
}

/** Whether there is anything to show. */
export const bgVisible = (tasks: TaskEv[]) => running(tasks).length > 0;

export function BgTasks({ tasks, openLog, stop }: BgProps) {
  const now = useNow();
  const list = running(tasks);
  if (!list.length) return null;
  return (
    <div className="cw-bgl compact">
      {list.map((t) => {
        const h = t.recentLines;
        const cur = h[h.length - 1];
        return (
          <div
            key={t.id}
            className={clsx("cw-tlf-row", t.agent ? "agent" : "shell")}
            onClick={() => openLog(t.id)}
            title={t.description ? `${t.description}\nClick to show its output` : "Show its output"}
          >
            <div className="cw-tlf-top">
              <span className="cw-tlf-icon">
                {t.agent ? <Bot size={11} /> : <SquareTerminal size={11} />}
              </span>
              <b>{t.title || t.description || t.id}</b>
              <span className="cw-tlf-kind">{t.agent ? (t.model ? `${t.agent} · ${modelName(t.model, {})}` : t.agent) : "shell"}</span>
              {!t.agent && <span className="cw-tlf-line">{t.last || "starting…"}</span>}
              <span className="cw-bg-time">{clock(now - t.startedAt)}</span>
              <StopButton t={t} stop={stop} />
            </div>
            {t.agent && (
              <div className="cw-tlf-track">
                {h.map((l, i) => (
                  <span
                    key={i}
                    className={clsx("cw-tlf-step", i === h.length - 1 && "cur")}
                    title={l}
                  >
                    <ToolIcon line={l} />
                  </span>
                ))}
                <span className="cw-tlf-cur">{cur ?? "starting…"}</span>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
