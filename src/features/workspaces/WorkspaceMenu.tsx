import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useEffect, useRef } from "react";
import { execute } from "@/commands/bus";
import { useWorkspaces, type Workspace } from "@/stores/workspaces";
import { getHome } from "@/lib/home";

/** Right-click menu for a workspace. Removing it is an explicit menu choice. */
export function WorkspaceMenu({ ws, x, y, onClose }: { ws: Workspace; x: number; y: number; onClose: () => void }) {
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const close = (e: MouseEvent) => !root.current?.contains(e.target as Node) && onClose();
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", esc);
    };
  }, [onClose]);

  const items: { label: string; run: () => void; danger?: boolean }[] = [
    { label: "Open", run: () => useWorkspaces.getState().setActiveWorkspace(ws.id) },
    { label: "Reveal folder in Finder", run: () => void revealItemInDir(ws.rootDir.replace(/^~(?=\/|$)/, getHome())).catch(() => {}) },
    { label: "Remove workspace…", run: () => execute({ type: "workspace.remove", id: ws.id }), danger: true },
  ];

  return (
    <div
      ref={root}
      style={{ left: x, top: y }}
      className="fixed z-50 w-52 rounded-lg border border-border-strong bg-panel-raised p-1 shadow-xl"
    >
      <div className="truncate px-2 py-1 text-[11px] text-fg-subtle">{ws.rootDir}</div>
      {items.map((item) => (
        <button
          key={item.label}
          onClick={() => {
            onClose();
            item.run();
          }}
          className={`block w-full rounded-md px-2 py-1.5 text-left text-[13px] hover:bg-hover ${item.danger ? "text-danger" : "text-fg"}`}
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}
