import { render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const protocol = vi.hoisted(() => ({
  mapRender: vi.fn(() => new Promise<Blob>(() => undefined)),
}));
vi.mock("./mapProtocol", async () => ({
  ...(await vi.importActual("./mapProtocol")),
  mapRender: protocol.mapRender,
}));

import { MapCanvas } from "./MapCanvas";
import type { RowSpan, SavedSelection } from "./mapProtocol";

const rows = (top: number, bottom: number): RowSpan[] =>
  Array.from({ length: bottom - top }, (_, index) => ({
    y: top + index,
    spans: [[4, 8]] as [number, number][],
  }));

const selection: SavedSelection = {
  id: "sel-1",
  label: "언덕",
  sourceRevision: "r1",
  role: "target",
  layers: [],
  bounds: { left: 4, top: 2, right: 8, bottom: 4 },
  selectedCells: 8,
  rows: rows(2, 4),
  snapshotHash: "a".repeat(64),
};

const baseProps = {
  renderSource: { key: "candidate|r1:hash", render: protocol.mapRender },
  ariaLabel: "맵 캔버스",
  width: 64,
  height: 48,
  view: "candidate" as const,
  layers: ["terrain" as const],
  selections: [selection],
  activeCells: new Set<string>(["6,6", "7,6"]),
  selectionShape: "rectangle" as const,
  selectionOperation: "replace" as const,
  interactionMode: "select" as const,
  objects: [],
  diffRows: [],
  diffMarkers: [],
  onActiveCells: vi.fn(),
  onCursor: vi.fn(),
  onObjectSelect: vi.fn(),
  onZoom: vi.fn(),
  onSelectionAnchor: vi.fn(),
};

describe("MapCanvas — saved selection overlays", () => {
  const originalResizeObserver = globalThis.ResizeObserver;
  let rect: { mockRestore(): void };
  let getContext: { mockRestore(): void };
  let calls: { fillRect: number[][]; stroke: number };

  beforeEach(() => {
    calls = { fillRect: [], stroke: 0 };
    const context = new Proxy({} as CanvasRenderingContext2D, {
      get(_target, key) {
        if (key === "fillRect") return (...args: number[]) => calls.fillRect.push(args);
        if (key === "stroke") return () => (calls.stroke += 1);
        return () => undefined;
      },
      set() {
        return true;
      },
    });
    getContext = vi
      .spyOn(HTMLCanvasElement.prototype, "getContext")
      .mockImplementation(() => context as never);
    rect = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockImplementation(() => ({
        x: 0,
        y: 0,
        top: 0,
        left: 0,
        right: 800,
        bottom: 600,
        width: 800,
        height: 600,
        toJSON: () => ({}),
      }) as DOMRect);
    globalThis.ResizeObserver = class ImmediateResizeObserver {
      constructor(private readonly callback: ResizeObserverCallback) {}
      observe(target: Element): void {
        this.callback(
          [{ target, contentRect: { width: 800, height: 600 } } as ResizeObserverEntry],
          this as unknown as ResizeObserver,
        );
      }
      disconnect(): void {}
      unobserve(): void {}
    } as unknown as typeof ResizeObserver;
  });

  afterEach(() => {
    getContext.mockRestore();
    rect.mockRestore();
    globalThis.ResizeObserver = originalResizeObserver;
  });

  it("fills saved and live selections by default, keeping only outlines when transparent", () => {
    vi.useFakeTimers();
    try {
      const { rerender } = render(<MapCanvas {...baseProps} />);
      vi.advanceTimersByTime(50);
      // Background, one fill per saved-selection row, plus the live mask row.
      const filled = calls.fillRect.length;
      const stroked = calls.stroke;
      expect(filled).toBeGreaterThanOrEqual(2 + selection.rows.length);
      expect(stroked).toBeGreaterThanOrEqual(1);

      calls.fillRect = [];
      calls.stroke = 0;
      rerender(<MapCanvas {...baseProps} selectionFill={false} />);
      vi.advanceTimersByTime(50);
      // Both the saved selection's rows and the live mask's row lose their fill.
      expect(calls.fillRect.length).toBe(filled - selection.rows.length - 1);
      // Every boundary outline is still stroked.
      expect(calls.stroke).toBe(stroked);
    } finally {
      vi.useRealTimers();
    }
  });
});
