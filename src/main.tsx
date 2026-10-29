import React from "react";
import ReactDOM from "react-dom/client";
import "./styles/globals.css";

async function main() {
  // Plain-browser dev mode (no Tauri runtime): use a fake backend.
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) await import("./dev/mockTauri");
  // An import applied at start brings the other machine's localStorage; restore it before
  // anything reads it.
  const { restoreImportedLocalStorage } = await import("./features/settings/transfer");
  await restoreImportedLocalStorage();
  // Imported after the mock so modules that listen at load time see the (fake) backend.
  const { default: App } = await import("./App");

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}

void main();
