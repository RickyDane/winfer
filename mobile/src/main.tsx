import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles.css";

// Service workers need a secure context (https or localhost). Not in dev: its cache would
// serve stale modules over Vite's.
if ("serviceWorker" in navigator && isSecureContext && import.meta.env.PROD) {
  navigator.serviceWorker.register("./sw.js").catch(() => {});
  navigator.serviceWorker.addEventListener("message", (ev) => {
    if (ev.data?.type === "open-pane") location.hash = ev.data.paneId ? `#/pane/${encodeURIComponent(ev.data.paneId)}` : "#/";
  });
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
