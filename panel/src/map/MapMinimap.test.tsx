import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const protocol = vi.hoisted(() => ({
  mapRender: vi.fn(() => new Promise<Blob>(() => undefined)),
}));
vi.mock("./mapProtocol", async () => ({
  ...(await vi.importActual("./mapProtocol")),
  mapRender: protocol.mapRender,
}));

import {
  fitMinimapGeometry,
  MapMinimap,
  minimapScreenToTile,
} from "./MapMinimap";

const renderSource = {
  key: "candidate|r1:hash",
  render: protocol.mapRender,
};
const baseProps = {
  renderSource,
  width: 100,
  height: 50,
  view: "candidate" as const,
  layers: ["terrain"] as const,
  selections: [],
  activeRows: [],
  objects: [],
  diffRows: [],
  diffMarkers: [],
  viewport: { left: 10, top: 10, right: 30, bottom: 20 },
};

describe("MapMinimap", () => {
  const originalResizeObserver = globalThis.ResizeObserver;
  let rect: { mockRestore(): void };

  beforeEach(() => {
    protocol.mapRender.mockClear();
    rect = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockImplementation(() => ({
        x: 10,
        y: 20,
        top: 20,
        left: 10,
        right: 210,
        bottom: 120,
        width: 200,
        height: 100,
        toJSON: () => ({}),
      }) as DOMRect);
    globalThis.ResizeObserver = class ImmediateResizeObserver {
      constructor(private readonly callback: ResizeObserverCallback) {}
      observe(target: Element): void {
        this.callback(
          [{ target, contentRect: { width: 200, height: 100 } } as ResizeObserverEntry],
          this as unknown as ResizeObserver,
        );
      }
      disconnect(): void {}
      unobserve(): void {}
    } as unknown as typeof ResizeObserver;
  });

  afterEach(() => {
    rect.mockRestore();
    globalThis.ResizeObserver = originalResizeObserver;
  });

  it("letterboxes the whole map and converts pointer positions to tile centers", () => {
    const geometry = fitMinimapGeometry(200, 100, 100, 100);
    expect(geometry).toEqual({ left: 50, top: 0, width: 100, height: 100, scale: 1 });
    expect(minimapScreenToTile({ x: 100, y: 50 }, geometry, 100, 100)).toEqual({
      x: 50,
      y: 50,
    });
    expect(minimapScreenToTile({ x: 0, y: 200 }, geometry, 100, 100)).toEqual({
      x: 0,
      y: 100,
    });
  });

  it("navigates by click/drag and offers Arrow/Shift/Home keyboard alternatives", () => {
    const onNavigate = vi.fn();
    render(<MapMinimap {...baseProps} layers={["terrain"]} onNavigate={onNavigate} />);
    const canvas = screen.getByLabelText("미니맵 — 클릭하거나 드래그하여 메인 캔버스 이동");

    fireEvent.pointerDown(canvas, { pointerId: 1, clientX: 110, clientY: 70 });
    fireEvent.pointerMove(canvas, { pointerId: 1, clientX: 150, clientY: 90 });
    fireEvent.pointerUp(canvas, { pointerId: 1, clientX: 150, clientY: 90 });
    fireEvent.keyDown(canvas, { key: "ArrowRight", shiftKey: true });
    fireEvent.keyDown(canvas, { key: "Home" });

    expect(protocol.mapRender).toHaveBeenCalledWith(
      expect.objectContaining({
        x: 0,
        y: 0,
        width: 100,
        height: 50,
        scale: 8,
        layers: ["terrain"],
      }),
    );
    expect(onNavigate).toHaveBeenCalledWith(28, 15);
    expect(onNavigate).toHaveBeenCalledWith(50, 25);
  });

  it("renders a request-owned draft and labels it as uncommitted work", () => {
    vi.useFakeTimers();
    try {
      render(
        <MapMinimap
          {...baseProps}
          renderSource={{ ...renderSource, key: "draft|map-request|2" }}
          view="draft"
          onNavigate={vi.fn()}
        />,
      );
      vi.advanceTimersByTime(120);

      expect(protocol.mapRender).toHaveBeenCalledWith(
        expect.objectContaining({
          x: 0,
          y: 0,
          width: 100,
          height: 50,
        }),
      );
      expect(screen.getByText("수정 중")).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });
});
