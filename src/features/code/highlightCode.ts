export interface HighlightRequest {
  id: number;
  code: string;
  lang: string;
  dark: boolean;
}

export interface HighlightResponse {
  id: number;
  html: string | null;
}

/** One lazy worker for all panes. Bound both DOM expansion and retained HTML;
 * oversized/unsupported snippets remain selectable, copyable plain text. */
export function createCodeHighlighter(makeWorker: () => Worker) {
  let worker: Worker | undefined;
  let unavailable = false;
  let seq = 0;
  const pending = new Map<number, (html: string | null) => void>();
  const cache = new Map<string, { result: Promise<string | null>; size: number }>();
  let chars = 0;
  const trim = () => {
    while (cache.size > 64 || chars > 2_000_000) {
      const oldest = cache.keys().next().value!;
      chars -= cache.get(oldest)!.size;
      cache.delete(oldest);
    }
  };
  const fail = () => {
    unavailable = true;
    worker?.terminate();
    worker = undefined;
    for (const resolve of pending.values()) resolve(null);
    pending.clear();
    cache.clear();
    chars = 0;
  };

  return (code: string, lang: string, dark: boolean): Promise<string | null> => {
    if (unavailable || !lang || /^(text|txt|plain|plaintext)$/i.test(lang) ||
        code.length > 50_000 || code.split("\n").length > 1000)
      return Promise.resolve(null);
    const key = `${dark}\0${lang}\0${code}`;
    const hit = cache.get(key);
    if (hit) {
      cache.delete(key);
      cache.set(key, hit);
      return hit.result;
    }
    try {
      if (!worker) {
        worker = makeWorker();
        worker.onmessage = ({ data }: MessageEvent<HighlightResponse>) => {
          pending.get(data.id)?.(data.html);
          pending.delete(data.id);
        };
        worker.onerror = fail;
        worker.onmessageerror = fail;
      }
      const id = ++seq;
      const result = new Promise<string | null>((resolve) => pending.set(id, resolve));
      const entry = { result, size: key.length };
      cache.set(key, entry);
      chars += key.length;
      trim();
      void result.then((html) => {
        if (cache.get(key) !== entry) return;
        entry.size += html?.length ?? 0;
        chars += html?.length ?? 0;
        trim();
      });
      worker.postMessage({ id, code, lang, dark } satisfies HighlightRequest);
      return result;
    } catch {
      fail();
      return Promise.resolve(null);
    }
  };
}

export const highlightCode = createCodeHighlighter(() =>
  new Worker(new URL("./highlightCode.worker.ts", import.meta.url), { type: "module" }),
);
