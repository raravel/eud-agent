import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { MapWorkbench } from "./MapWorkbench";

describe("Map workbench selection popover", () => {
  it("moves by drag and keyboard while staying inside the canvas", async () => {
    const originalResizeObserver = globalThis.ResizeObserver;
    const rect = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockImplementation(function (this: HTMLElement) {
        const isPopover = this.dataset.slot === "selection-toolbar-popover";
        const isMain = this.tagName === "MAIN";
        const width = isPopover ? 400 : isMain ? 800 : 0;
        const height = isPopover ? 220 : isMain ? 600 : 0;
        return {
          x: 0,
          y: 0,
          top: 0,
          left: 0,
          right: width,
          bottom: height,
          width,
          height,
          toJSON: () => ({}),
        } as DOMRect;
      });
    globalThis.ResizeObserver = class ImmediateResizeObserver {
      constructor(private readonly callback: ResizeObserverCallback) {}
      observe(target: Element): void {
        this.callback([{ target } as ResizeObserverEntry], this as unknown as ResizeObserver);
      }
      disconnect(): void {}
      unobserve(): void {}
    } as unknown as typeof ResizeObserver;

    try {
      const { container } = render(
        <MapWorkbench
          toolbar={<div>toolbar</div>}
          palette={<div>palette</div>}
          minimap={<div>minimap</div>}
          canvas={<div>canvas</div>}
          agent={<div>agent</div>}
          selectionToolbar={<div>selection controls</div>}
          status={<div>status</div>}
          selectionAnchor={{ x: 400, y: 300 }}
        />,
      );
      const popover = container.querySelector<HTMLElement>(
        '[data-slot="selection-toolbar-popover"]',
      );
      expect(popover).not.toBeNull();
      await waitFor(() => expect(popover?.style.left).toBe("200px"));

      const handle = screen.getByRole("button", { name: "선택 패널 이동" });
      fireEvent.keyDown(handle, { key: "ArrowRight" });
      await waitFor(() => expect(popover?.style.left).toBe("216px"));

      fireEvent.pointerDown(handle, { pointerId: 1, clientX: 100, clientY: 100 });
      fireEvent.pointerMove(handle, { pointerId: 1, clientX: 132, clientY: 124 });
      fireEvent.pointerUp(handle, { pointerId: 1, clientX: 132, clientY: 124 });
      await waitFor(() => {
        expect(popover?.style.left).toBe("248px");
        expect(popover?.style.top).toBe("96px");
      });
    } finally {
      rect.mockRestore();
      globalThis.ResizeObserver = originalResizeObserver;
    }
  });
});

describe("Map workbench sidebar resizing", () => {
  it("allows both sidebars to reach their expanded maximum widths", () => {
    localStorage.removeItem("map-agent.layout/1");
    const { container } = render(
      <MapWorkbench
        toolbar={<div>toolbar</div>}
        palette={<div>palette</div>}
        minimap={<div>minimap</div>}
        canvas={<div>canvas</div>}
        agent={<div>agent</div>}
        selectionToolbar={<div>selection controls</div>}
        status={<div>status</div>}
        selectionAnchor={null}
      />,
    );
    const grid = container.querySelector<HTMLElement>(".map-workbench > .grid");
    const leftSplitter = screen.getByRole("separator", { name: "팔레트 너비 조절" });
    const rightSplitter = screen.getByRole("separator", { name: "에이전트 패널 너비 조절" });

    for (let step = 0; step < 30; step += 1) {
      fireEvent.keyDown(leftSplitter, { key: "ArrowRight" });
      fireEvent.keyDown(rightSplitter, { key: "ArrowLeft" });
    }

    expect(grid?.style.gridTemplateColumns).toBe(
      "640px 5px minmax(0,1fr) 5px 720px",
    );
  });

  it("splits the left sidebar vertically and persists keyboard resizing", async () => {
    localStorage.removeItem("map-agent.layout/1");
    const { container } = render(
      <MapWorkbench
        toolbar={<div>toolbar</div>}
        palette={<div>palette</div>}
        minimap={<div>minimap content</div>}
        canvas={<div>canvas</div>}
        agent={<div>agent</div>}
        selectionToolbar={<div>selection controls</div>}
        status={<div>status</div>}
        selectionAnchor={null}
      />,
    );
    const splitter = screen.getByRole("separator", {
      name: "팔레트와 미니맵 높이 조절",
    });
    const pane = container.querySelector<HTMLElement>(
      '[data-slot="map-minimap-pane"]',
    );
    expect(screen.getByText("palette")).toBeInTheDocument();
    expect(screen.getByText("minimap content")).toBeInTheDocument();
    expect(pane?.style.height).toBe("220px");

    fireEvent.keyDown(splitter, { key: "ArrowUp" });
    expect(pane?.style.height).toBe("236px");
    fireEvent.keyDown(splitter, { key: "ArrowDown" });
    expect(pane?.style.height).toBe("220px");

    await waitFor(() =>
      expect(JSON.parse(localStorage.getItem("map-agent.layout/1") ?? "{}")).toMatchObject({
        minimapHeight: 220,
      }),
    );
  });
});
