import { invoke } from "@tauri-apps/api/core";
import { ChevronLeft, ChevronRight, Minus, Plus } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { getDocument, GlobalWorkerOptions, type PDFDocumentLoadingTask, type PDFDocumentProxy, type RenderTask } from "pdfjs-dist/legacy/build/pdf.mjs";
import workerUrl from "pdfjs-dist/legacy/build/pdf.worker.min.mjs?url";
import { Spinner } from "@/components/Spinner";
import { fileName } from "./paths";

GlobalWorkerOptions.workerSrc = workerUrl;
const assets = `${import.meta.env.BASE_URL}pdfjs/`;
const buttonClass = "rounded p-1 text-fg-muted hover:bg-hover disabled:opacity-30 disabled:hover:bg-transparent";

/** Render locally rather than relying on PDF plugins (unavailable in some webviews). */
export default function PdfView({ path, revision }: { path: string; revision: number }) {
  const [pdf, setPdf] = useState<PDFDocumentProxy | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [page, setPage] = useState(1);
  const [zoom, setZoom] = useState(1);
  const [width, setWidth] = useState(0);
  const [rendering, setRendering] = useState(false);
  const container = useRef<HTMLDivElement>(null);
  const surface = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let cancelled = false;
    let task: PDFDocumentLoadingTask | undefined;
    setPdf(null);
    setError(null);
    void (async () => {
      try {
        const bytes = await invoke<ArrayBuffer>("files_read_bytes", { path });
        if (cancelled) return;
        task = getDocument({
          data: new Uint8Array(bytes),
          cMapUrl: `${assets}cmaps/`,
          standardFontDataUrl: `${assets}standard_fonts/`,
          wasmUrl: `${assets}wasm/`,
          iccUrl: `${assets}iccs/`,
        });
        const doc = await task.promise;
        if (cancelled) return;
        setPage((p) => Math.min(p, doc.numPages));
        setPdf(doc);
      } catch (err) {
        if (!cancelled) setError(String(err));
      }
    })();
    return () => {
      cancelled = true;
      void task?.destroy().catch(() => {});
    };
  }, [path, revision]);

  useEffect(() => {
    const box = container.current;
    if (!box) return;
    const observer = new ResizeObserver(([entry]) => setWidth(Math.max(1, entry.contentRect.width - 32)));
    observer.observe(box);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const box = surface.current;
    if (!pdf || !box || !width) return;
    let cancelled = false;
    let task: RenderTask | undefined;
    // Each render owns its canvas: rapid page/zoom changes cannot reuse a busy canvas.
    const canvas = document.createElement("canvas");
    canvas.setAttribute("role", "img");
    canvas.setAttribute("aria-label", `${fileName(path)}, page ${page} of ${pdf.numPages}`);
    canvas.className = "block bg-white shadow-md";
    setRendering(true);
    setError(null);
    void (async () => {
      try {
        const current = await pdf.getPage(page);
        if (cancelled) return;
        const natural = current.getViewport({ scale: 1 });
        const viewport = current.getViewport({ scale: width / natural.width * zoom });
        // Bound bitmap memory, including on high-DPI displays and very large pages.
        const ratio = Math.min(window.devicePixelRatio || 1, 4096 / Math.max(viewport.width, viewport.height), Math.sqrt(16_000_000 / (viewport.width * viewport.height)));
        canvas.width = Math.max(1, Math.floor(viewport.width * ratio));
        canvas.height = Math.max(1, Math.floor(viewport.height * ratio));
        canvas.style.width = `${viewport.width}px`;
        canvas.style.height = `${viewport.height}px`;
        box.replaceChildren(canvas);
        task = current.render({ canvas, viewport, transform: [ratio, 0, 0, ratio, 0, 0] });
        await task.promise;
      } catch (err) {
        if (!cancelled) setError(String(err));
      } finally {
        if (!cancelled) setRendering(false);
      }
    })();
    return () => {
      cancelled = true;
      task?.cancel();
      canvas.remove();
      canvas.width = canvas.height = 0;
    };
  }, [pdf, path, page, zoom, width]);

  return (
    <div ref={container} className="flex h-full min-h-0 flex-col">
      {pdf && (
        <div className="flex shrink-0 items-center justify-center gap-2 border-b border-border px-2 py-1 text-xs text-fg-muted" aria-label="PDF controls">
          <button title="Previous page" className={buttonClass} disabled={page <= 1} onClick={() => setPage((p) => p - 1)}><ChevronLeft size={14} /></button>
          <span aria-live="polite">Page {page} of {pdf.numPages}</span>
          <button title="Next page" className={buttonClass} disabled={page >= pdf.numPages} onClick={() => setPage((p) => p + 1)}><ChevronRight size={14} /></button>
          <span className="mx-1 h-3 border-l border-border" />
          <button title="Zoom out" className={buttonClass} disabled={zoom <= 0.25} onClick={() => setZoom((z) => Math.max(0.25, z - 0.25))}><Minus size={14} /></button>
          <button title="Fit to width" className="rounded px-1 py-0.5 hover:bg-hover" onClick={() => setZoom(1)}>{Math.round(zoom * 100)}%</button>
          <button title="Zoom in" className={buttonClass} disabled={zoom >= 3} onClick={() => setZoom((z) => Math.min(3, z + 0.25))}><Plus size={14} /></button>
        </div>
      )}
      {error && <div role="alert" className="p-4 text-center text-xs text-danger">Couldn't display PDF: {error}</div>}
      {!error && (!pdf || rendering) && <div role="status" aria-label="Loading PDF" className="flex justify-center p-3"><Spinner size={14} /></div>}
      <div className="min-h-0 flex-1 overflow-auto p-4">
        <div ref={surface} className="mx-auto w-fit" />
      </div>
    </div>
  );
}
