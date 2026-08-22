import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { GripHorizontal, PanelLeftClose, PanelLeftOpen, PanelRightClose, PanelRightOpen } from "lucide-react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

const LEFT_SIDEBAR_MAX_WIDTH = 640;
const RIGHT_SIDEBAR_MAX_WIDTH = 720;

export interface MapWorkbenchProps {
  toolbar: ReactNode;
  palette: ReactNode;
  minimap: ReactNode;
  canvas: ReactNode;
  agent: ReactNode;
  selectionToolbar: ReactNode;
  status: ReactNode;
  selectionAnchor: { x: number; y: number } | null;
}

interface SavedLayout {
  leftWidth?: number;
  rightWidth?: number;
  leftOpen?: boolean;
  rightOpen?: boolean;
  minimapHeight?: number;
}

function loadLayout(): SavedLayout {
  try {
    return JSON.parse(localStorage.getItem("map-agent.layout/1") ?? "{}") as SavedLayout;
  } catch {
    return {};
  }
}

export function MapWorkbench({
  toolbar,
  palette,
  minimap,
  canvas,
  agent,
  selectionToolbar,
  status,
  selectionAnchor,
}: MapWorkbenchProps) {
  const saved = useMemo(loadLayout, []);
  const [leftWidth, setLeftWidth] = useState(saved.leftWidth ?? 280);
  const [rightWidth, setRightWidth] = useState(saved.rightWidth ?? 390);
  const [leftOpen, setLeftOpen] = useState(saved.leftOpen ?? true);
  const [rightOpen, setRightOpen] = useState(saved.rightOpen ?? true);
  const [minimapHeight, setMinimapHeight] = useState(saved.minimapHeight ?? 220);
  const mainRef = useRef<HTMLElement>(null);
  const floatingToolbarRef = useRef<HTMLDivElement>(null);
  const [mainSize, setMainSize] = useState({ width: 1, height: 1 });
  const [floatingToolbarSize, setFloatingToolbarSize] = useState({ width: 1, height: 1 });
  const [floatingPosition, setFloatingPosition] = useState<{ left: number; top: number } | null>(null);

  useEffect(() => {
    localStorage.setItem(
      "map-agent.layout/1",
      JSON.stringify({ leftWidth, rightWidth, leftOpen, rightOpen, minimapHeight }),
    );
  }, [leftOpen, leftWidth, minimapHeight, rightOpen, rightWidth]);

  useEffect(() => {
    const main = mainRef.current;
    const floating = floatingToolbarRef.current;
    if (!main || !floating) return;
    const observer = new ResizeObserver(() => {
      const mainRect = main.getBoundingClientRect();
      const toolbarRect = floating.getBoundingClientRect();
      setMainSize({ width: mainRect.width, height: mainRect.height });
      setFloatingToolbarSize({ width: toolbarRect.width, height: toolbarRect.height });
    });
    observer.observe(main);
    observer.observe(floating);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (!selectionAnchor) setFloatingPosition(null);
  }, [selectionAnchor]);

  const clampFloatingPosition = useCallback(
    (position: { left: number; top: number }) => ({
      left: Math.max(
        8,
        Math.min(mainSize.width - floatingToolbarSize.width - 8, position.left),
      ),
      top: Math.max(
        8,
        Math.min(mainSize.height - floatingToolbarSize.height - 8, position.top),
      ),
    }),
    [floatingToolbarSize.height, floatingToolbarSize.width, mainSize.height, mainSize.width],
  );

  const automaticFloatingPosition = useMemo(() => {
    if (!selectionAnchor) return null;
    const above = selectionAnchor.y - floatingToolbarSize.height - 8;
    return clampFloatingPosition({
      left: selectionAnchor.x - floatingToolbarSize.width / 2,
      top: above >= 8 ? above : selectionAnchor.y + 8,
    });
  }, [
    clampFloatingPosition,
    floatingToolbarSize.height,
    floatingToolbarSize.width,
    selectionAnchor,
  ]);
  const resolvedFloatingPosition = floatingPosition
    ? clampFloatingPosition(floatingPosition)
    : automaticFloatingPosition;

  const startFloatingDrag = useCallback(
    (event: React.PointerEvent<HTMLButtonElement>) => {
      if (!resolvedFloatingPosition) return;
      event.preventDefault();
      const target = event.currentTarget;
      const start = { x: event.clientX, y: event.clientY };
      const initial = resolvedFloatingPosition;
      target.setPointerCapture(event.pointerId);
      const move = (pointer: PointerEvent) => {
        setFloatingPosition(
          clampFloatingPosition({
            left: initial.left + pointer.clientX - start.x,
            top: initial.top + pointer.clientY - start.y,
          }),
        );
      };
      const finish = () => {
        target.removeEventListener("pointermove", move);
        target.removeEventListener("pointerup", finish);
        target.removeEventListener("pointercancel", finish);
      };
      target.addEventListener("pointermove", move);
      target.addEventListener("pointerup", finish);
      target.addEventListener("pointercancel", finish);
    },
    [clampFloatingPosition, resolvedFloatingPosition],
  );

  const moveFloatingByKeyboard = useCallback(
    (event: React.KeyboardEvent<HTMLButtonElement>) => {
      if (!resolvedFloatingPosition) return;
      const delta =
        event.key === "ArrowLeft"
          ? { x: -16, y: 0 }
          : event.key === "ArrowRight"
            ? { x: 16, y: 0 }
            : event.key === "ArrowUp"
              ? { x: 0, y: -16 }
              : event.key === "ArrowDown"
                ? { x: 0, y: 16 }
                : null;
      if (!delta) return;
      event.preventDefault();
      setFloatingPosition(
        clampFloatingPosition({
          left: resolvedFloatingPosition.left + delta.x,
          top: resolvedFloatingPosition.top + delta.y,
        }),
      );
    },
    [clampFloatingPosition, resolvedFloatingPosition],
  );

  const clampMinimapHeight = useCallback(
    (height: number) => {
      if (mainSize.height <= 1) return height;
      return Math.max(140, Math.min(Math.max(140, mainSize.height - 220), height));
    },
    [mainSize.height],
  );

  useEffect(() => {
    setMinimapHeight((height) => clampMinimapHeight(height));
  }, [clampMinimapHeight]);

  const startMinimapResize = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      const start = event.clientY;
      const initial = minimapHeight;
      const target = event.currentTarget;
      target.setPointerCapture(event.pointerId);
      const move = (pointer: PointerEvent) => {
        setMinimapHeight(clampMinimapHeight(initial + start - pointer.clientY));
      };
      const finish = () => {
        target.removeEventListener("pointermove", move);
        target.removeEventListener("pointerup", finish);
        target.removeEventListener("pointercancel", finish);
      };
      target.addEventListener("pointermove", move);
      target.addEventListener("pointerup", finish);
      target.addEventListener("pointercancel", finish);
    },
    [clampMinimapHeight, minimapHeight],
  );

  const startResize = useCallback(
    (side: "left" | "right", event: React.PointerEvent<HTMLDivElement>) => {
      const start = event.clientX;
      const initial = side === "left" ? leftWidth : rightWidth;
      const target = event.currentTarget;
      target.setPointerCapture(event.pointerId);
      const move = (pointer: PointerEvent) => {
        const delta = pointer.clientX - start;
        const width = side === "left" ? initial + delta : initial - delta;
        const clamped = Math.max(
          side === "left" ? 220 : 320,
          Math.min(
            side === "left" ? LEFT_SIDEBAR_MAX_WIDTH : RIGHT_SIDEBAR_MAX_WIDTH,
            width,
          ),
        );
        if (side === "left") setLeftWidth(clamped);
        else setRightWidth(clamped);
      };
      const finish = () => {
        target.removeEventListener("pointermove", move);
        target.removeEventListener("pointerup", finish);
        target.removeEventListener("pointercancel", finish);
      };
      target.addEventListener("pointermove", move);
      target.addEventListener("pointerup", finish);
      target.addEventListener("pointercancel", finish);
    },
    [leftWidth, rightWidth],
  );

  return (
    <div className="map-workbench flex h-dvh min-h-0 min-w-0 flex-col overflow-hidden bg-background text-foreground">
      {toolbar}
      <div
        className="relative grid min-h-0 min-w-0 flex-1 overflow-hidden"
        style={{
          gridTemplateColumns: `${leftOpen ? `${leftWidth}px 5px` : "0 0"} minmax(0,1fr) ${rightOpen ? `5px ${rightWidth}px` : "0 0"}`,
        }}
      >
        <div
          className={cn(
            "flex min-h-0 min-w-0 flex-col overflow-hidden",
            !leftOpen && "invisible",
          )}
        >
          <div className="min-h-0 min-w-0 flex-1 overflow-hidden">{palette}</div>
          <div
            role="separator"
            aria-label="팔레트와 미니맵 높이 조절"
            aria-orientation="horizontal"
            aria-valuemin={140}
            aria-valuemax={Math.max(140, Math.floor(mainSize.height - 220))}
            aria-valuenow={Math.round(minimapHeight)}
            tabIndex={leftOpen ? 0 : -1}
            className="h-1.5 shrink-0 cursor-row-resize bg-border transition-colors hover:bg-cyan-500 focus:bg-cyan-500 focus:outline-none"
            title="드래그하거나 위/아래 방향키로 미니맵 높이 조절"
            onPointerDown={startMinimapResize}
            onKeyDown={(event) => {
              if (event.key === "ArrowUp") {
                event.preventDefault();
                setMinimapHeight((height) => clampMinimapHeight(height + 16));
              }
              if (event.key === "ArrowDown") {
                event.preventDefault();
                setMinimapHeight((height) => clampMinimapHeight(height - 16));
              }
              if (event.key === "Home") {
                event.preventDefault();
                setMinimapHeight(clampMinimapHeight(220));
              }
            }}
          />
          <div
            data-slot="map-minimap-pane"
            className="min-h-[140px] min-w-0 shrink-0 overflow-hidden"
            style={{ height: minimapHeight }}
          >
            {minimap}
          </div>
        </div>
        <div
          role="separator"
          aria-label="팔레트 너비 조절"
          aria-orientation="vertical"
          tabIndex={leftOpen ? 0 : -1}
          className="cursor-col-resize bg-border hover:bg-primary focus:bg-primary focus:outline-none"
          onPointerDown={(event) => startResize("left", event)}
          onKeyDown={(event) => {
            if (event.key === "ArrowLeft") setLeftWidth((width) => Math.max(220, width - 16));
            if (event.key === "ArrowRight") {
              setLeftWidth((width) => Math.min(LEFT_SIDEBAR_MAX_WIDTH, width + 16));
            }
          }}
        />
        <main ref={mainRef} className="relative min-h-0 min-w-0 overflow-hidden">
          {canvas}
          {selectionAnchor && resolvedFloatingPosition ? (
            <div
              ref={floatingToolbarRef}
              data-slot="selection-toolbar-popover"
              className="pointer-events-none absolute z-20 w-[min(48rem,calc(100%-7rem))]"
              style={{
                left: resolvedFloatingPosition.left,
                top: resolvedFloatingPosition.top,
              }}
            >
              <div className="pointer-events-auto flex justify-center">
                <Button
                  type="button"
                  size="sm"
                  variant="secondary"
                  className="-mb-px h-7 w-16 cursor-grab touch-none rounded-b-none border border-border px-2 active:cursor-grabbing"
                  aria-label="선택 패널 이동"
                  title="드래그하거나 방향키로 이동 · 더블 클릭하면 자동 위치"
                  onPointerDown={startFloatingDrag}
                  onKeyDown={moveFloatingByKeyboard}
                  onDoubleClick={() => setFloatingPosition(null)}
                >
                  <GripHorizontal className="size-4" aria-hidden="true" />
                </Button>
              </div>
              <div className="pointer-events-auto min-w-0 w-full">{selectionToolbar}</div>
            </div>
          ) : (
            <div
              ref={floatingToolbarRef}
              className="pointer-events-none absolute inset-x-14 bottom-3 z-20 flex justify-center"
            >
              <div className="pointer-events-auto min-w-0 w-full max-w-3xl">{selectionToolbar}</div>
            </div>
          )}
          <Button
            type="button"
            size="icon"
            variant="secondary"
            className="absolute left-2 top-2 z-30 min-h-11 min-w-11"
            aria-label={leftOpen ? "팔레트와 미니맵 접기" : "팔레트와 미니맵 열기"}
            onClick={() => setLeftOpen((open) => !open)}
          >
            {leftOpen ? <PanelLeftClose className="size-4" /> : <PanelLeftOpen className="size-4" />}
          </Button>
          <Button
            type="button"
            size="icon"
            variant="secondary"
            className="absolute right-2 top-2 z-30 min-h-11 min-w-11"
            aria-label={rightOpen ? "에이전트 패널 접기" : "에이전트 패널 열기"}
            onClick={() => setRightOpen((open) => !open)}
          >
            {rightOpen ? <PanelRightClose className="size-4" /> : <PanelRightOpen className="size-4" />}
          </Button>
        </main>
        <div
          role="separator"
          aria-label="에이전트 패널 너비 조절"
          aria-orientation="vertical"
          tabIndex={rightOpen ? 0 : -1}
          className="cursor-col-resize bg-border hover:bg-primary focus:bg-primary focus:outline-none"
          onPointerDown={(event) => startResize("right", event)}
          onKeyDown={(event) => {
            if (event.key === "ArrowLeft") {
              setRightWidth((width) => Math.min(RIGHT_SIDEBAR_MAX_WIDTH, width + 16));
            }
            if (event.key === "ArrowRight") setRightWidth((width) => Math.max(320, width - 16));
          }}
        />
        <div className={cn("min-h-0 min-w-0 overflow-hidden", !rightOpen && "invisible")}>{agent}</div>
      </div>
      <footer className="flex min-h-9 min-w-0 items-center overflow-hidden border-t border-border bg-card/80 px-3">
        {status}
      </footer>
    </div>
  );
}
