import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles/globals.css";

async function main() {
  // Plain-browser dev mode (no Tauri runtime): use a fake backend.
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) await import("./dev/mockTauri");

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}

void main();
