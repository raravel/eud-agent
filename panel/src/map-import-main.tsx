import { Component, StrictMode, type ErrorInfo, type ReactNode } from "react";
import { createRoot } from "react-dom/client";

import "@/index.css";
import MapImportApp from "@/map/MapImportApp";

class MapImportErrorBoundary extends Component<
  { children: ReactNode },
  { error: string }
> {
  state = { error: "" };

  static getDerivedStateFromError(error: unknown) {
    return { error: String(error) };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Map Importer surface failed", error, info);
  }

  render() {
    if (this.state.error) {
      return (
        <main className="grid h-dvh place-items-center overflow-hidden bg-background p-6 text-foreground">
          <section className="max-w-xl rounded-xl border border-destructive/50 bg-card p-6">
            <h1 className="text-lg font-semibold">Map Importer 화면 오류</h1>
            <p className="mt-2 break-words text-sm text-destructive">
              {this.state.error}
            </p>
          </section>
        </main>
      );
    }
    return this.props.children;
  }
}

document.documentElement.classList.add("dark");
const root = document.getElementById("root");
if (!root) throw new Error("Map Importer root element is missing");

createRoot(root).render(
  <StrictMode>
    <MapImportErrorBoundary>
      <MapImportApp />
    </MapImportErrorBoundary>
  </StrictMode>,
);
