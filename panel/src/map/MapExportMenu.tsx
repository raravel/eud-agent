import { useEffect, useState } from "react";
import { ClipboardCopy, Download, ImageDown, LoaderCircle } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  mapExportImage,
  mapExportImageSave,
  type MapExportImageCommand,
} from "./mapProtocol";

type ExportStatus =
  | { kind: "idle" }
  | { kind: "busy"; target: "clipboard" | "file" }
  | { kind: "done"; message: string }
  | { kind: "error"; message: string };

/** Clipboard/PNG export of the map picture shown on the canvas. */
export function MapExportMenu({ command }: { command: MapExportImageCommand }) {
  const [status, setStatus] = useState<ExportStatus>({ kind: "idle" });
  const busy = status.kind === "busy";

  useEffect(() => {
    if (status.kind !== "done") return;
    const timer = window.setTimeout(() => setStatus({ kind: "idle" }), 4000);
    return () => window.clearTimeout(timer);
  }, [status]);

  const copy = async () => {
    setStatus({ kind: "busy", target: "clipboard" });
    try {
      // A pending blob keeps the write inside this click's activation even
      // though a large map takes a while to draw.
      const png = mapExportImage(command);
      await navigator.clipboard.write([new ClipboardItem({ "image/png": png })]);
      await png;
      setStatus({ kind: "done", message: "맵 이미지를 클립보드에 복사했습니다." });
    } catch (reason) {
      setStatus({
        kind: "error",
        message: `클립보드에 복사하지 못했습니다. 창을 클릭한 뒤 다시 시도하거나 PNG로 저장해 주세요. (${String(reason)})`,
      });
    }
  };

  const save = async () => {
    setStatus({ kind: "busy", target: "file" });
    try {
      const path = await mapExportImageSave(command);
      setStatus(
        path === null
          ? { kind: "idle" }
          : { kind: "done", message: `맵 이미지를 저장했습니다: ${path}` },
      );
    } catch (reason) {
      setStatus({
        kind: "error",
        message: `맵 이미지를 저장하지 못했습니다. 다시 시도해 주세요. (${String(reason)})`,
      });
    }
  };

  return (
    <div className="flex items-center gap-2">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={busy}
            className="h-9"
            title="지형·두데드·스프라이트·유닛·건물을 타일당 32px 원본 크기로 내보내기"
          >
            {busy ? (
              <LoaderCircle
                className="size-4 animate-spin motion-reduce:animate-none"
                aria-hidden="true"
              />
            ) : (
              <ImageDown className="size-4" aria-hidden="true" />
            )}
            {busy ? "이미지 만드는 중…" : "이미지 내보내기"}
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem onSelect={() => void copy()}>
            <ClipboardCopy className="size-4" aria-hidden="true" />
            클립보드에 복사
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => void save()}>
            <Download className="size-4" aria-hidden="true" />
            PNG로 저장…
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      {(status.kind === "done" || status.kind === "error") && (
        <span
          role={status.kind === "error" ? "alert" : "status"}
          className={
            status.kind === "error"
              ? "max-w-72 truncate text-xs text-destructive"
              : "max-w-72 truncate text-xs text-emerald-300"
          }
          title={status.message}
        >
          {status.message}
        </span>
      )}
    </div>
  );
}
