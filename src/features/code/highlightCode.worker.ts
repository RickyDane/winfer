import { codeToHtml } from "shiki";
import type { HighlightRequest, HighlightResponse } from "./highlightCode";

// Serialize work so a restored transcript cannot start hundreds of tokenizations
// concurrently. All grammar loading and regex execution stay off the UI thread.
let queue = Promise.resolve();
self.onmessage = ({ data }: MessageEvent<HighlightRequest>) => {
  queue = queue.then(async () => {
    let html: string | null = null;
    try {
      html = await codeToHtml(data.code, {
        lang: data.lang,
        theme: data.dark ? "github-dark-default" : "github-light-default",
        tokenizeMaxLineLength: 2000,
      });
    } catch {
      // Unknown languages or unavailable grammars keep the plain-text fallback.
    }
    self.postMessage({ id: data.id, html } satisfies HighlightResponse);
  });
};
