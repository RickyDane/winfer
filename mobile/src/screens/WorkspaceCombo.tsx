// The Panel tab's workspace picker: a combobox whose list opens under it with a search field,
// "All workspaces" (like the desktop's Overview) first, then each workspace.
import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import { haptic } from "../hooks";
import type { Workspace } from "../protocol";
import { Icon } from "./common";

/** The "All workspaces" choice. */
export const ALL = "__all__";

const label = (w: Workspace) => (w.machineName ? `${w.name} · ${w.machineName}` : w.name);

/** Workspaces matching `query` (name or machine, any word order). */
export function matchWorkspaces(workspaces: Workspace[], query: string): Workspace[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  return workspaces.filter((w) => words.every((q) => label(w).toLowerCase().includes(q)));
}

export function WorkspaceCombo({ workspaces, value, onChange }: { workspaces: Workspace[]; value: string; onChange: (id: string) => void }) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [cursor, setCursor] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLUListElement>(null);
  const id = useId();

  const found = matchWorkspaces(workspaces, query);
  // "All" stays offered unless the search names something else.
  const options: string[] = [...(query.trim() ? [] : [ALL]), ...found.map((w) => w.id)];
  const current = workspaces.find((w) => w.id === value);

  useEffect(() => {
    if (!open) return;
    const away = (e: PointerEvent) => !root.current?.contains(e.target as Node) && setOpen(false);
    document.addEventListener("pointerdown", away);
    return () => document.removeEventListener("pointerdown", away);
  }, [open]);
  useEffect(() => {
    setCursor(Math.max(0, options.indexOf(value)));
  }, [open, query]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    list.current?.querySelector('[data-cursor="true"]')?.scrollIntoView({ block: "nearest" });
  }, [cursor, open]);

  const pick = (wsId: string) => {
    haptic(6);
    onChange(wsId);
    setOpen(false);
    setQuery("");
  };
  const toggle = () => {
    haptic(4);
    setOpen((o) => !o);
    setQuery("");
  };
  const keys = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown") setCursor((c) => Math.min(options.length - 1, c + 1));
    else if (e.key === "ArrowUp") setCursor((c) => Math.max(0, c - 1));
    else if (e.key === "Enter" && options[cursor]) pick(options[cursor]);
    else if (e.key === "Escape") setOpen(false);
    else return;
    e.preventDefault();
  };

  return (
    <div className="ws-combo" ref={root} data-open={open || undefined}>
      <button type="button" className="ws-combo-trigger" aria-haspopup="listbox" aria-expanded={open} aria-controls={`${id}-list`} onClick={toggle}>
        <span className="ws-combo-icon">
          <Icon name={value === ALL ? "layers" : "folder"} size={16} />
        </span>
        <span className="row-main">
          <b>{value === ALL ? "All workspaces" : (current?.name ?? "Pick a workspace")}</b>
          {value === ALL ? <span>{workspaces.length} workspaces, like the desktop's overview</span> : current?.machineName && <span>{current.machineName}</span>}
        </span>
        <Icon name="chevron" size={16} className="ws-combo-chevron" />
      </button>
      {open && (
        <div className="ws-combo-pop">
          <div className="pick-search ws-combo-search">
            <Icon name="search" size={15} />
            <input
              autoFocus
              role="combobox"
              aria-expanded
              aria-controls={`${id}-list`}
              aria-activedescendant={options[cursor] ? `${id}-${cursor}` : undefined}
              aria-label="Search workspaces"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={keys}
              placeholder="Search workspaces"
              inputMode="search"
              enterKeyHint="go"
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
            />
            {query && (
              <button type="button" className="icon-btn" aria-label="Clear search" onClick={() => setQuery("")}>
                <Icon name="x" size={15} />
              </button>
            )}
          </div>
          <ul className="ws-combo-list" role="listbox" id={`${id}-list`} aria-label="Workspaces" ref={list}>
            {options.map((o, i) => {
              const w = workspaces.find((x) => x.id === o);
              return (
                <li
                  key={o}
                  id={`${id}-${i}`}
                  role="option"
                  aria-selected={o === value}
                  data-cursor={i === cursor}
                  className="ws-combo-option"
                  onPointerEnter={() => setCursor(i)}
                  onClick={() => pick(o)}
                >
                  <Icon name={o === ALL ? "layers" : "folder"} size={15} />
                  <span className="row-main">
                    <b>{o === ALL ? "All workspaces" : w?.name}</b>
                    {(o === ALL || w?.machineName) && <span>{o === ALL ? "Every workspace's tabs, files and terminals" : w?.machineName}</span>}
                  </span>
                  {o === value && <Icon name="check" size={15} className="ws-combo-check" />}
                </li>
              );
            })}
            {!options.length && <li className="ws-combo-none">No workspace matches “{query.trim()}”</li>}
          </ul>
        </div>
      )}
    </div>
  );
}
