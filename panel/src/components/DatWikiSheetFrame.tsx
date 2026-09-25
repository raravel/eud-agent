/**
 * One frame of a StarCraft GRP sheet, drawn out of the single grid image the
 * Rust side renders.
 *
 * A sheet is fetched once per session and shared by every list row, inline
 * value and header that needs a frame of it — the icon sheet alone carries 390
 * frames, so a request per thumbnail would be hundreds of round trips for one
 * scroll. Frames are sliced with `background-position`, which is why the grid's
 * geometry (`columns`, `frameWidth`, `frameHeight`) travels with the image.
 */
import { useEffect, useState } from "react";

import { datWikiSheet, type DatWikiSheet, type DatWikiSheetImage } from "@/lib/ipc";
import { cn } from "@/lib/utils";

/** One in-flight or settled request per sheet, shared across components. */
const sheets = new Map<DatWikiSheet, Promise<DatWikiSheetImage>>();

/** Drops the cache so a test (or a project switch) starts from nothing. */
export function resetDatWikiSheets(): void {
  sheets.clear();
}

function loadSheet(sheet: DatWikiSheet): Promise<DatWikiSheetImage> {
  const pending = sheets.get(sheet);
  if (pending !== undefined) return pending;
  const request = datWikiSheet(sheet);
  sheets.set(sheet, request);
  // A failed sheet must not be cached as a permanent failure: the install can
  // be configured while the app is open.
  request.catch(() => sheets.delete(sheet));
  return request;
}

/**
 * The sheet's grid image, once it has arrived. A sheet that cannot be read is
 * simply absent — every caller draws a plain number instead, so a missing
 * StarCraft install costs pictures and nothing else.
 */
export function useDatWikiSheet(sheet: DatWikiSheet | undefined): DatWikiSheetImage | null {
  const [image, setImage] = useState<DatWikiSheetImage | null>(null);
  useEffect(() => {
    if (sheet === undefined) {
      setImage(null);
      return;
    }
    let current = true;
    loadSheet(sheet).then(
      (loaded) => current && setImage(loaded),
      () => current && setImage(null),
    );
    return () => {
      current = false;
    };
  }, [sheet]);
  return image;
}

export interface DatWikiSheetFrameProps {
  sheet: DatWikiSheet;
  frame: number;
  /** The longest edge of the drawn box, in pixels. */
  size: number;
  /** What the picture shows, for anyone not looking at it. */
  label: string;
  className?: string;
}

export function DatWikiSheetFrame({
  sheet,
  frame,
  size,
  label,
  className,
}: DatWikiSheetFrameProps) {
  const image = useDatWikiSheet(sheet);
  if (image === null || frame < 0 || frame >= image.frames) {
    // A reserved box keeps the rows aligned whether or not a frame exists.
    return (
      <span
        aria-hidden="true"
        className={cn("block shrink-0", className)}
        style={{ width: size, height: size }}
      />
    );
  }
  const scale = size / Math.max(image.frameWidth, image.frameHeight);
  const rows = Math.ceil(image.frames / image.columns);
  const column = frame % image.columns;
  const row = Math.floor(frame / image.columns);
  return (
    <span
      role="img"
      aria-label={label}
      className={cn("block shrink-0", className)}
      style={{
        width: image.frameWidth * scale,
        height: image.frameHeight * scale,
        backgroundImage: `url(${image.png})`,
        backgroundSize: `${image.columns * image.frameWidth * scale}px ${rows * image.frameHeight * scale}px`,
        backgroundPosition: `-${column * image.frameWidth * scale}px -${row * image.frameHeight * scale}px`,
        backgroundRepeat: "no-repeat",
        imageRendering: "pixelated",
      }}
    />
  );
}
