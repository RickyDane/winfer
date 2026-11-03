import { Component, type ReactNode } from "react";
import { logger } from "./logger";

/**
 * Last-resort render guard: a React crash would otherwise white-screen the
 * app and take every terminal's view with it. The error (with component
 * stack) goes to the file log; the user gets a reload that keeps the Rust
 * side — and the agents behind the panes — running.
 */
export class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: { componentStack?: string | null }) {
    logger.error("react", error.message, error.stack ?? "", info.componentStack ?? "");
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 bg-panel px-6 text-center">
        <p className="text-sm font-medium text-fg">winfer's interface crashed</p>
        <p className="max-w-md text-xs leading-relaxed text-fg-muted">
          Your agents keep running — reloading only restarts the interface. The error was written to the log
          (Settings → Diagnostics), which helps if you report it.
        </p>
        <p className="max-w-md truncate font-mono text-[11px] text-danger">{this.state.error.message}</p>
        <button
          onClick={() => window.location.reload()}
          className="flex h-7 items-center rounded-md border border-border-strong bg-panel-raised px-2.5 text-xs text-fg hover:bg-hover"
        >
          Reload interface
        </button>
      </div>
    );
  }
}
