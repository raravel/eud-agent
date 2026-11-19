import i18n from "@/lib/i18n";

/** 1536 -> "1.5 KB". */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 100 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
}

/** 83_400 -> "1:23.4"; null -> "—". */
export function formatDuration(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return "—";
  const tenths = Math.round(ms / 100);
  const minutes = Math.floor(tenths / 600);
  const seconds = (tenths % 600) / 10;
  return `${minutes}:${seconds.toFixed(1).padStart(4, "0")}`;
}

/** "44.1 kHz · 2ch", leaving out what is unknown. */
export function formatSignal(sampleRate: number | null, channels: number | null): string {
  const parts: string[] = [];
  if (sampleRate) parts.push(`${(sampleRate / 1000).toFixed(sampleRate % 1000 === 0 ? 0 : 1)} kHz`);
  if (channels) {
    parts.push(
      channels === 1
        ? i18n.t("sound.mono")
        : channels === 2
          ? i18n.t("sound.stereo")
          : `${channels}ch`,
    );
  }
  return parts.join(" · ");
}
