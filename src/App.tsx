import { AppShell } from "@/app/AppShell";
import { ErrorBoundary } from "@/features/diagnostics/ErrorBoundary";

export default function App() {
  return (
    <ErrorBoundary>
      <AppShell />
    </ErrorBoundary>
  );
}
