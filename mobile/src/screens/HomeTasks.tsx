import { useState } from "react";
import { ago, go, haptic, setFromTask } from "../hooks";
import type { HomeTask, Pane } from "../protocol";
import { Avatar, STATUS_LABEL } from "./common";
import { Sheet } from "./Pickers";
import { STAGE_LABEL, noAgentsNote, taskAgents, taskHow, taskOpen } from "./tasks";

function open(p: Pane) {
  setFromTask(p.id);
  go(`/pane/${encodeURIComponent(p.id)}`);
}

/** Each Home's task list. A task with one agent opens it; otherwise a sheet shows the task and what there is to open. */
export function HomeTasks({ homes, panes }: { homes: Pane[]; panes: Pane[] }) {
  const [picked, setPicked] = useState<string | null>(null);
  // Looked up again each render, so an open sheet follows the task (and closes when it's gone).
  const home = homes.find((h) => h.tasks?.some((t) => t.id === picked));
  const sheet = home?.tasks?.find((t) => t.id === picked);
  const agents = home && sheet ? taskAgents(sheet, home, panes) : [];

  const tap = (h: Pane, t: HomeTask) => {
    haptic(6);
    const found = taskAgents(t, h, panes);
    if (found.length === 1) open(found[0]);
    else setPicked(t.id);
  };

  return (
    <>
      {homes.map((h) => {
        // With several desktops each list says whose it is.
        const whose = homes.length > 1 && h.machineName;
        return (
          <section key={h.id} className="list-group">
            <h2 className="label">
              {whose ? `Tasks · ${h.machineName}` : "Tasks"}
              {h.tasks && <span>{h.tasks.filter(taskOpen).length}</span>}
            </h2>
            {!h.tasks ? (
              <p className="group-foot">Update winfer on your computer to see Home's tasks here.</p>
            ) : !h.tasks.length ? (
              <p className="group-foot">Ask Home for something to build: it becomes a task here.</p>
            ) : (
              <div className="task-list">
                {h.tasks.map((t) => (
                  <button key={t.id} className="task" data-done={!taskOpen(t) || undefined} onClick={() => tap(h, t)}>
                    <span className="task-top">
                      <span className="task-n">Task {t.number}</span>
                      <span className="task-stage" data-stage={t.stage}>
                        {STAGE_LABEL[t.stage] ?? t.stage}
                      </span>
                      <span className="time">{ago(t.updatedAt)}</span>
                    </span>
                    <span className="task-goal">{t.goal}</span>
                    <span className="card-meta">
                      <span className="ws">{t.workspace}</span>
                      <span className="sep">·</span>
                      <span className="prov">{taskHow(t, taskAgents(t, h, panes).length)}</span>
                    </span>
                    {t.summary && <span className="task-summary">{t.summary}</span>}
                  </button>
                ))}
              </div>
            )}
          </section>
        );
      })}
      {sheet && (
        <Sheet title={`Task ${sheet.number} · ${STAGE_LABEL[sheet.stage] ?? sheet.stage}`} sub={sheet.workspace} label={`Task ${sheet.number}`} onClose={() => setPicked(null)}>
          <div className="task-detail">
            <p>{sheet.goal}</p>
            {sheet.summary && <p className="sub">{sheet.summary}</p>}
            {!agents.length && <p className="sub">{noAgentsNote(sheet)}</p>}
          </div>
          {agents.length > 0 && (
            <div className="sheet-body">
              {agents.map((p) => (
                <button key={p.id} className="menu-item" onClick={() => open(p)}>
                  <Avatar pane={p} small />
                  <span className="task-agent">{p.title}</span>
                  <span className="menu-value">{STATUS_LABEL[p.status]}</span>
                </button>
              ))}
            </div>
          )}
        </Sheet>
      )}
    </>
  );
}
