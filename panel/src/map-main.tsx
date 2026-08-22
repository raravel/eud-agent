import { Component, StrictMode } from "react";
import type { ErrorInfo, ReactNode } from "react";
import { createRoot } from "react-dom/client";

import "@/index.css";
import MapAgentApp from "@/map/MapAgentApp";

class MapSurfaceErrorBoundary extends Component<
  { children: ReactNode },
  { error: string }
> {
  state = { error: "" };

  static getDerivedStateFromError(error: unknown) {
    return { error: String(error) };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Map Agent surface failed", error, info);
  }

  render() {
    if (this.state.error) {
      return (
        <main className="grid h-dvh place-items-center bg-background p-6 text-foreground">
          <section className="max-w-xl rounded-xl border border-destructive/50 bg-card p-6">
            <h1 className="text-lg font-semibold">Map Agent 화면 오류</h1>
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
if (!root) throw new Error("Map Agent root element is missing");

createRoot(root).render(
  <StrictMode>
    <MapSurfaceErrorBoundary>
      <MapAgentApp />
    </MapSurfaceErrorBoundary>
  </StrictMode>,
);
