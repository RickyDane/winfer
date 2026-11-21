import { invoke } from "@tauri-apps/api/core";
import { Check, ChevronDown, CornerDownLeft, Search, Sparkles, X } from "lucide-react";
import type { Attachment } from "@/features/code/store";
import { useEffect, useMemo, useRef, useState } from "react";
import { execute } from "@/commands/bus";
import { EffortIcon, EFFORT_HINTS, EFFORT_LABELS } from "@/features/code/EffortIcon";
import { backendOf, modelName } from "@/features/code/names";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { fetchModels, type ProviderId } from "@/features/providers/providers";
import { cn } from "@/lib/cn";
import { defaultProviderId, providerName, useActiveWorkspace, useWorkspaces } from "@/stores/workspaces";

interface HarnessModel {
  spec: string;
  label: string;
  detail: string;
  efforts?: string[];
}

interface WelcomeModel {
  id: string;
  label: string;
  backend?: string;
  efforts?: string[];
}

const HARNESS_BACKENDS = new Set(["claude", "codex", "agy", "cursor", "opencode", "pi", "muse"]);

const BACKEND_DEFAULT_EFFORTS: Record<string, string[]> = {
  winfer: ["low", "medium", "high", "xhigh", "max"],
  claude: ["low", "medium", "high", "xhigh", "max"],
  codex: ["low", "medium", "high", "xhigh"],
  muse: ["low", "medium", "high"],
  pi: ["minimal", "low", "medium", "high", "xhigh"],
};

let cachedHarnessModels: HarnessModel[] | null = null;

/**
 * The empty workspace (Graphite "Welcome"): a task box with backend, model and effort pickers,
 * the installed agents to switch between, and a few next steps. Port of the native `welcome.rs`.
 */
export function Welcome() {
  const workspace = useActiveWorkspace();
  const providers = useWorkspaces((s) => s.providers);
  const agentModels = useWorkspaces((s) => s.agentModels);
  const defaultId = useWorkspaces((s) => defaultProviderId(s));
  const [selectedProvider, setSelectedProvider] = useState<ProviderId | null>(null);
  const [selectedModel, setSelectedModel] = useState<string | null>(null);
  const [selectedEffort, setSelectedEffort] = useState<string | null>(null);
  const [openPicker, setOpenPicker] = useState<"model" | "effort" | null>(null);
  const [modelQuery, setModelQuery] = useState("");
  const [task, setTask] = useState("");
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [harnessModels, setHarnessModels] = useState<HarnessModel[] | null>(cachedHarnessModels);
  const [providerModels, setProviderModels] = useState<Record<string, WelcomeModel[]>>({});
  const [loadingProvider, setLoadingProvider] = useState<string | null>(null);
  const pickerRef = useRef<HTMLDivElement>(null);

  const agents = providers.filter((p) => p.id !== "shell" && p.installed);
  const missing = providers.filter((p) => p.id !== "shell" && !p.installed).length;
  const activeProvider: ProviderId =
    selectedProvider && providers.some((p) => p.id === selectedProvider && p.installed)
      ? selectedProvider
      : providers.some((p) => p.id === "winfer" && p.installed)
        ? "winfer"
        : defaultId;
  const activeProviderInfo = providers.find((p) => p.id === activeProvider);

  useEffect(() => {
    if (cachedHarnessModels) return;
    if (activeProvider !== "winfer" && !HARNESS_BACKENDS.has(activeProvider)) return;
    let alive = true;
    void invoke<HarnessModel[]>("harness_models")
      .then((list) => {
        cachedHarnessModels = list;
        if (alive) setHarnessModels(list);
      })
      .catch(() => {
        if (alive) setHarnessModels([]);
      });
    return () => {
      alive = false;
    };
  }, [activeProvider]);

  useEffect(() => {
    if (activeProvider === "winfer" || !activeProviderInfo?.modelArg || providerModels[activeProvider]) return;
    let alive = true;
    setLoadingProvider(activeProvider);
    void fetchModels(activeProvider)
      .then((list) => {
        if (alive) setProviderModels((prev) => ({ ...prev, [activeProvider]: list.map((m) => ({ id: m.id, label: m.label })) }));
      })
      .catch(() => {
        if (alive) setProviderModels((prev) => ({ ...prev, [activeProvider]: [] }));
      })
      .finally(() => {
        if (alive) setLoadingProvider((cur) => (cur === activeProvider ? null : cur));
      });
    return () => {
      alive = false;
    };
  }, [activeProvider, activeProviderInfo?.modelArg, providerModels]);

  useEffect(() => {
    if (!openPicker) return;
    const onDown = (e: MouseEvent) => {
      if (!pickerRef.current?.contains(e.target as Node)) setOpenPicker(null);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [openPicker]);

  const models: WelcomeModel[] = useMemo(() => {
    if (activeProvider === "winfer") {
      return (harnessModels ?? []).map((m) => ({
        id: m.spec,
        label: modelName(m.spec, {}),
        backend: backendOf(m.spec)?.id,
        efforts: m.efforts,
      }));
    }
    if (HARNESS_BACKENDS.has(activeProvider) && harnessModels) {
      const prefix = `${activeProvider}/`;
      const matched = harnessModels
        .filter((m) => m.spec.startsWith(prefix))
        .map((m) => ({ id: m.spec.slice(prefix.length), label: m.label, efforts: m.efforts }));
      if (matched.length > 0) return matched;
    }
    return providerModels[activeProvider] ?? [];
  }, [activeProvider, harnessModels, providerModels]);

  const modelsLoading =
    (activeProvider === "winfer" && harnessModels === null) ||
    (HARNESS_BACKENDS.has(activeProvider) && harnessModels === null && !providerModels[activeProvider]) ||
    loadingProvider === activeProvider;

  const efforts: string[] = useMemo(() => {
    const found = selectedModel ? models.find((m) => m.id === selectedModel) : undefined;
    if (found) {
      if (found.efforts) return found.efforts.length === 0 ? [] : ["default", ...found.efforts];
      const fallback = BACKEND_DEFAULT_EFFORTS[activeProvider];
      return fallback ? ["default", ...fallback] : [];
    }
    const fallback = BACKEND_DEFAULT_EFFORTS[activeProvider];
    return fallback ? ["default", ...fallback] : [];
  }, [activeProvider, models, selectedModel]);

  const savedModel = agentModels[activeProvider] ?? null;
  const effectiveModel = selectedModel ?? savedModel;
  const modelLabel = effectiveModel
    ? (models.find((m) => m.id === effectiveModel)?.label ?? (activeProvider === "winfer" ? modelName(effectiveModel, {}) : effectiveModel))
    : "Default model";
  const modelIconProvider =
    activeProvider === "winfer" ? (effectiveModel ? backendOf(effectiveModel)?.logo ?? "winfer" : "winfer") : activeProvider;
  const activeEffort = selectedEffort && efforts.includes(selectedEffort) ? selectedEffort : "default";
  const showModelPicker = !!activeProviderInfo?.modelArg || models.length > 0;
  const showEffortPicker = efforts.length > 0;

  const filteredModels = useMemo(() => {
    const q = modelQuery.trim().toLowerCase();
    if (!q) return models.slice(0, 80);
    return models.filter((m) => m.label.toLowerCase().includes(q) || m.id.toLowerCase().includes(q)).slice(0, 80);
  }, [models, modelQuery]);

  const selectBackend = (id: ProviderId) => {
    if (activeProvider !== id) {
      setSelectedProvider(id);
      setSelectedModel(null);
      setSelectedEffort(null);
      setOpenPicker(null);
      setModelQuery("");
    }
  };

  // Only winfer code sessions can take images with their first message.
  const canAttach = activeProvider === "winfer";
  const addImages = (files: File[]) => {
    for (const f of files) {
      const reader = new FileReader();
      reader.onload = () =>
        setAttachments((a) => [...a, { id: `${Date.now()}-${Math.random()}`, url: String(reader.result), mime: f.type, name: f.name || "pasted image" }]);
      reader.readAsDataURL(f);
    }
  };

  // Paste anywhere on the welcome screen (the task box isn't focused by default, so an input-level handler never fires).
  const addImagesRef = useRef(addImages);
  addImagesRef.current = addImages;
  useEffect(() => {
    if (!canAttach) return;
    const onPaste = (e: ClipboardEvent) => {
      const t = e.target as HTMLElement | null;
      // Leave other text fields (model search, etc.) alone; the task box is fine.
      if (t && (t.tagName === "TEXTAREA" || (t.tagName === "INPUT" && t.getAttribute("aria-label") !== "Task for a new agent"))) return;
      const data = e.clipboardData;
      if (!data) return;
      const files = data.files.length ? [...data.files] : [...data.items].map((i) => (i.kind === "file" ? i.getAsFile() : null)).filter((f): f is File => !!f);
      const images = files.filter((f) => f.type.startsWith("image/"));
      if (images.length) {
        e.preventDefault();
        addImagesRef.current(images);
      }
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, [canAttach]);

  const start = () => {
    const text = task.trim();
    const images = canAttach ? attachments : [];
    setOpenPicker(null);
    setTask("");
    setAttachments([]);
    execute({
      type: "pane.spawn",
      provider: activeProvider,
      prompt: text || undefined,
      attachments: images.length ? images : undefined,
      model: selectedModel ?? undefined,
      effort: activeEffort !== "default" ? activeEffort : undefined,
    });
  };

  const primary = agents.find((p) => p.id === "winfer");
  const others = agents.filter((p) => p.id !== "winfer");

  const agentButton = (p: (typeof agents)[number], big: boolean) => {
    const on = p.id === activeProvider;
    return (
      <button
        key={p.id}
        onClick={() => selectBackend(p.id)}
        onDoubleClick={() => {
          selectBackend(p.id);
          execute({ type: "pane.spawn", provider: p.id });
        }}
        className={cn(
          "flex items-center justify-center gap-2 rounded-2xl font-medium shadow-sm transition-all",
          big ? "h-9 min-w-[120px] px-4 text-[13.5px]" : "h-9 min-w-[80px] px-3 text-[13px]",
          on ? "bg-accent-soft text-fg" : "bg-panel-raised text-fg-muted hover:bg-hover hover:text-fg",
        )}
      >
        <ProviderIcon provider={p.id} size={big ? 18 : 15} />
        {big ? "Winfer code" : p.name}
      </button>
    );
  };

  return (
    <div className="flex h-full w-full items-center justify-center rounded-[var(--radius-pane)] border border-border bg-panel shadow-sm">
      <div className="flex w-[620px] max-w-[92%] flex-col items-center gap-5">
        <span className="grid size-11 place-items-center rounded-xl bg-accent-soft text-accent">
          <Sparkles size={20} />
        </span>
        <div className="text-center">
          <h2 className="text-xl font-semibold text-fg">What should agents do in {workspace?.name ?? "this workspace"}?</h2>
          <p className="mt-1 text-[13px] text-fg-muted">One line starts an agent in its own worktree. Or pick an agent below.</p>
        </div>
        <div className="flex w-full flex-col rounded-xl bg-panel-raised focus-within:ring-2 focus-within:ring-accent/40 focus-within:ring-3 focus-within:ring-accent-soft shadow-sm">
          {canAttach && attachments.length > 0 && (
            <div className="flex flex-wrap gap-2 px-3.5 pt-2.5">
              {attachments.map((a) => (
                <div key={a.id} className="relative" title={a.name}>
                  <img src={a.url} alt={a.name} className="size-12 rounded-md object-cover" />
                  <button
                    type="button"
                    onClick={() => setAttachments((all) => all.filter((x) => x.id !== a.id))}
                    title="Remove"
                    className="absolute -top-1 -right-1 grid size-4 place-items-center rounded-full bg-fg text-bg"
                  >
                    <X size={10} />
                  </button>
                </div>
              ))}
            </div>
          )}
          <label className="flex h-[46px] w-full items-center gap-2 pr-1.5 pl-3.5">
            <ProviderIcon provider={activeProvider} size={16} />
            <input
              value={task}
              onChange={(e) => setTask(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && start()}
              placeholder="Describe a task — Enter starts an agent on it"
              aria-label="Task for a new agent"
              className="min-w-0 flex-1 bg-transparent text-sm text-fg outline-none placeholder:text-fg-subtle"
            />
            <button onClick={start} className="flex h-8 shrink-0 items-center gap-1.5 rounded-lg bg-accent px-3 text-[12.5px] font-medium text-on-accent hover:opacity-90">
              Start {providerName(providers, activeProvider)}
              <CornerDownLeft size={13} />
            </button>
          </label>
          {(showModelPicker || showEffortPicker) && (
            <div ref={pickerRef} className="flex items-center gap-1.5 px-2.5 py-1.5">
              {showModelPicker && (
                <div className="relative">
                  <button
                    type="button"
                    onClick={() => {
                      setOpenPicker((p) => (p === "model" ? null : "model"));
                      setModelQuery("");
                    }}
                    title="Choose model for this agent"
                    className={cn(
                      "flex h-[26px] items-center gap-1.5 rounded-md px-2 text-xs text-fg-muted transition-colors hover:bg-hover hover:text-fg",
                      openPicker === "model" && "bg-hover text-fg",
                    )}
                  >
                    <ProviderIcon provider={modelIconProvider} size={13} />
                    <span className="max-w-[200px] truncate">{modelLabel}</span>
                    <ChevronDown size={11} className="text-fg-subtle" />
                  </button>
                  {openPicker === "model" && (
                    <div className="anim-menu absolute top-[32px] left-0 z-30 w-[290px] overflow-hidden rounded-xl border border-border-strong bg-panel-raised shadow-lg">
                      {models.length > 8 && (
                        <div className="flex items-center gap-2 px-2.5 py-1.5">
                          <Search size={12} className="text-fg-subtle" />
                          <input
                            autoFocus
                            value={modelQuery}
                            onChange={(e) => setModelQuery(e.target.value)}
                            placeholder="Filter models…"
                            className="min-w-0 flex-1 bg-transparent text-xs text-fg outline-none placeholder:text-fg-subtle"
                          />
                        </div>
                      )}
                      <div className="flex max-h-[240px] flex-col gap-px overflow-y-auto p-1.5">
                        <button
                          type="button"
                          onClick={() => {
                            setSelectedModel(null);
                            setSelectedEffort(null);
                            setOpenPicker(null);
                          }}
                          className={cn(
                            "flex h-[30px] items-center gap-2 rounded-md px-2.5 text-left text-[12.5px]",
                            selectedModel === null ? "bg-accent/15 text-fg" : "text-fg-muted hover:bg-hover hover:text-fg",
                          )}
                        >
                          <span className="flex-1 truncate">
                            {savedModel ? `Default (${activeProvider === "winfer" ? modelName(savedModel, {}) : savedModel})` : "Default model"}
                          </span>
                          {selectedModel === null && <Check size={13} className="shrink-0 text-accent" />}
                        </button>
                        {modelsLoading && filteredModels.length === 0 && (
                          <div className="px-2.5 py-2 text-[11.5px] text-fg-subtle">Loading {providerName(providers, activeProvider)} models…</div>
                        )}
                        {filteredModels.map((m) => {
                          const on = selectedModel === m.id;
                          return (
                            <button
                              key={m.id}
                              type="button"
                              onClick={() => {
                                setSelectedModel(m.id);
                                if (selectedEffort && m.efforts && !m.efforts.includes(selectedEffort)) {
                                  setSelectedEffort(null);
                                }
                                setOpenPicker(null);
                              }}
                              className={cn(
                                "flex h-[30px] items-center gap-2 rounded-md px-2.5 text-left text-[12.5px]",
                                on ? "bg-accent/15 text-fg" : "text-fg-muted hover:bg-hover hover:text-fg",
                              )}
                            >
                              {m.backend && <ProviderIcon provider={backendOf(m.id)?.logo ?? m.backend} size={13} />}
                              <span className="min-w-0 flex-1 truncate">{m.label}</span>
                              {on && <Check size={13} className="shrink-0 text-accent" />}
                            </button>
                          );
                        })}
                      </div>
                    </div>
                  )}
                </div>
              )}
              {showEffortPicker && (
                <div className="relative">
                  <button
                    type="button"
                    onClick={() => setOpenPicker((p) => (p === "effort" ? null : "effort"))}
                    title={`Reasoning effort: ${EFFORT_LABELS[activeEffort] ?? activeEffort}`}
                    className={cn(
                      "flex h-[26px] items-center gap-1.5 rounded-md px-2 text-xs text-fg-muted transition-colors hover:bg-hover hover:text-fg",
                      openPicker === "effort" && "bg-hover text-fg",
                    )}
                  >
                    <EffortIcon effort={activeEffort} size={13} />
                    <span>{EFFORT_LABELS[activeEffort] ?? activeEffort}</span>
                    <ChevronDown size={11} className="text-fg-subtle" />
                  </button>
                  {openPicker === "effort" && (
                    <div className="anim-menu absolute top-[32px] left-0 z-30 w-[260px] overflow-hidden rounded-xl border border-border-strong bg-panel-raised p-1.5 shadow-lg">
                      <div className="flex flex-col gap-px">
                        {efforts.map((lvl) => {
                          const on = activeEffort === lvl;
                          const hint = EFFORT_HINTS[lvl];
                          return (
                            <button
                              key={lvl}
                              type="button"
                              onClick={() => {
                                setSelectedEffort(lvl === "default" ? null : lvl);
                                setOpenPicker(null);
                              }}
                              className={cn(
                                "flex h-8 items-center gap-2 rounded-md px-2.5 text-left text-[12.5px]",
                                on ? "bg-accent/15 text-fg" : "text-fg-muted hover:bg-hover hover:text-fg",
                              )}
                            >
                              <span className={cn("inline-flex shrink-0", on ? "text-accent" : "text-fg-muted")}>
                                <EffortIcon effort={lvl} size={14} />
                              </span>
                              <span className="flex min-w-0 flex-1 items-center gap-1.5">
                                <span className="shrink-0">{EFFORT_LABELS[lvl] ?? lvl}</span>
                                {hint && <span className="truncate text-[10.5px] text-fg-subtle">{hint}</span>}
                              </span>
                              {on && <Check size={13} className="shrink-0 text-accent" />}
                            </button>
                          );
                        })}
                      </div>
                    </div>
                  )}
                </div>
              )}
            </div>
          )}
        </div>
        <div className="flex w-full items-stretch justify-center gap-2.5">
          {primary && (
            <>
              {agentButton(primary, true)}
              {others.length > 0 && <span className="flex shrink-0 items-center px-1 text-[12px] text-fg-subtle">or</span>}
            </>
          )}
          <div className="flex flex-wrap items-stretch justify-center gap-2">{others.map((p) => agentButton(p, false))}</div>
        </div>
        {missing > 0 && (
          <button
            onClick={() => {
              execute({ type: "ui.settings", page: "agents" });
              // Once the page has rendered, scroll to the agents that can be installed.
              window.setTimeout(() => document.getElementById("agents-to-install")?.scrollIntoView({ block: "start", behavior: "smooth" }), 150);
            }}
            className="-mt-2 text-xs text-fg-subtle hover:text-fg"
          >
            + {missing} more agents you can install
          </button>
        )}
      </div>
    </div>
  );
}
