import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DatWikiGraphic } from "@/lib/ipc";

import { DatWikiAnimation, END_PAUSE_MS, TICK_MS } from "./DatWikiAnimation";

/** An intro frame, then a two-frame loop, unless `loopStart` says otherwise. */
function graphic(overrides: Partial<DatWikiGraphic> = {}): DatWikiGraphic {
  return {
    png: "data:image/png;base64,grid",
    frameWidth: 10,
    frameHeight: 10,
    columns: 3,
    frames: 3,
    grp: "unit\\test.grp",
    imageId: 1,
    iscriptId: 1,
    slots: ["Init"],
    slot: "Init",
    steps: [
      { cell: 0, ticks: 2, flip: false },
      { cell: 1, ticks: 1, flip: false },
      { cell: 2, ticks: 1, flip: true },
    ],
    loopStart: 1,
    ...overrides,
  };
}

function shown(): { cell: number; flipped: boolean } {
  const picture = screen.getByRole("img", { name: "그래픽" });
  return {
    cell: Number(picture.dataset.cell),
    flipped: picture.style.transform === "scaleX(-1)",
  };
}

function advance(ms: number) {
  act(() => {
    vi.advanceTimersByTime(ms);
  });
}

describe("DatWikiAnimation", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("holds each frame for its ticks and loops back to the loop start", () => {
    render(<DatWikiAnimation graphic={graphic()} scale={1} label="그래픽" />);

    expect(shown()).toEqual({ cell: 0, flipped: false });
    advance(2 * TICK_MS - 1);
    expect(shown().cell).toBe(0);
    advance(1);
    expect(shown()).toEqual({ cell: 1, flipped: false });
    advance(TICK_MS);
    expect(shown()).toEqual({ cell: 2, flipped: true });
    // The intro frame plays once: the loop starts at step 1.
    advance(TICK_MS);
    expect(shown().cell).toBe(1);
  });

  it("rests on the last frame of an ending animation, then plays it again", () => {
    render(
      <DatWikiAnimation graphic={graphic({ loopStart: undefined })} scale={1} label="그래픽" />,
    );

    advance(2 * TICK_MS);
    advance(TICK_MS);
    expect(shown().cell).toBe(2);
    advance(TICK_MS);
    expect(shown().cell).toBe(2);
    advance(END_PAUSE_MS - 1);
    expect(shown().cell).toBe(2);
    advance(1);
    expect(shown().cell).toBe(0);
  });

  it("pauses and resumes from its button", () => {
    render(<DatWikiAnimation graphic={graphic()} scale={1} label="그래픽" />);

    fireEvent.click(screen.getByRole("button", { name: "일시 정지" }));
    advance(10 * TICK_MS);
    expect(shown().cell).toBe(0);

    fireEvent.click(screen.getByRole("button", { name: "재생" }));
    advance(2 * TICK_MS);
    expect(shown().cell).toBe(1);
  });

  it("starts paused under reduced motion", () => {
    vi.stubGlobal(
      "matchMedia",
      (query: string) => ({ matches: query.includes("reduce") }) as MediaQueryList,
    );
    render(<DatWikiAnimation graphic={graphic()} scale={1} label="그래픽" />);

    advance(10 * TICK_MS);
    expect(shown().cell).toBe(0);
    expect(screen.getByRole("button", { name: "재생" })).toBeInTheDocument();
  });

  it("offers no playback for a graphic that never changes", () => {
    render(
      <DatWikiAnimation
        graphic={graphic({ steps: [{ cell: 0, ticks: 1, flip: false }], loopStart: 0 })}
        scale={2}
        label="그래픽"
      />,
    );

    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    expect(screen.getByRole("img", { name: "그래픽" }).style.width).toBe("20px");
  });
});
