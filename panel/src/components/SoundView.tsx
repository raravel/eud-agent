/**
 * The center 사운드 tab: every WAV registration of the saved source map, what
 * the map holds for it, and the original the project keeps, each playable.
 * Read-only; sounds change only through the agent's sound tools.
 */
import { useTranslation } from "react-i18next";
import { useCallback, useEffect, useRef, useState } from "react";
import { Music, Play, RefreshCw, Square } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { formatBytes, formatDuration, formatSignal } from "@/components/soundFormat";
import {
  soundPanelMapAudio,
  soundPanelOriginalAudio,
  type SoundAudioFacts,
  type SoundPanelEntry,
  type SoundPanelList,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";

export interface SoundFocus {
  mpqPath: string;
  nonce: number;
}

export interface SoundViewProps {
  list: SoundPanelList | null;
  loading: boolean;
  error: string | null;
  onRefresh(): void;
  focus: SoundFocus | null;
  /** Byte loaders; tests replace them. */
  loadMapAudio?(mpqPath: string): Promise<Uint8Array>;
  loadOriginal?(sha256: string): Promise<Uint8Array>;
}

type Track = { kind: "map"; mpqPath: string } | { kind: "original"; sha256: string };

function trackKey(track: Track): string {
  return track.kind === "map" ? `map:${track.mpqPath}` : `original:${track.sha256}`;
}

function mimeFor(format: string): string {
  const value = format.toLowerCase();
  if (value.includes("ogg") || value === "vorbis" || value === "opus") return "audio/ogg";
  if (value.includes("wav") || value.startsWith("pcm")) return "audio/wav";
  if (value === "mp3") return "audio/mpeg";
  if (value === "flac") return "audio/flac";
  return "application/octet-stream";
}

/** One shared player: starting a track stops the one before it. */
function useSoundPlayer(
  loadMapAudio: (mpqPath: string) => Promise<Uint8Array>,
  loadOriginal: (sha256: string) => Promise<Uint8Array>,
) {
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const urlRef = useRef<string | null>(null);
  const requestRef = useRef(0);
  const [playing, setPlaying] = useState<string | null>(null);
  const [pending, setPending] = useState<string | null>(null);
  const [elapsedMs, setElapsedMs] = useState(0);
  const [error, setError] = useState<{ key: string; message: string } | null>(null);

  const release = useCallback(() => {
    const audio = audioRef.current;
    if (audio) {
      audio.pause();
      audio.removeAttribute("src");
    }
    if (urlRef.current !== null) {
      URL.revokeObjectURL(urlRef.current);
      urlRef.current = null;
    }
  }, []);

  const stop = useCallback(() => {
    requestRef.current += 1;
    release();
    setPlaying(null);
    setPending(null);
    setElapsedMs(0);
  }, [release]);

  useEffect(() => stop, [stop]);

  const play = useCallback(
    async (track: Track, format: string) => {
      const key = trackKey(track);
      stop();
      const request = requestRef.current;
      setPending(key);
      setError(null);
      try {
        const bytes =
          track.kind === "map" ? await loadMapAudio(track.mpqPath) : await loadOriginal(track.sha256);
        if (request !== requestRef.current) return;
        const copy = new Uint8Array(bytes.byteLength);
        copy.set(bytes);
        const url = URL.createObjectURL(new Blob([copy.buffer], { type: mimeFor(format) }));
        urlRef.current = url;
        const audio = audioRef.current ?? new Audio();
        audioRef.current = audio;
        audio.ontimeupdate = () => setElapsedMs(Math.round(audio.currentTime * 1000));
        audio.onended = () => {
          if (request === requestRef.current) stop();
        };
        audio.src = url;
        await audio.play();
        if (request !== requestRef.current) return;
        setPlaying(key);
      } catch (cause) {
        if (request !== requestRef.current) return;
        release();
        setError({ key, message: String(cause) });
      } finally {
        if (request === requestRef.current) setPending(null);
      }
    },
    [loadMapAudio, loadOriginal, release, stop],
  );

  return { playing, pending, elapsedMs, error, play, stop };
}

export function SoundView({
  list,
  loading,
  error,
  onRefresh,
  focus,
  loadMapAudio = soundPanelMapAudio,
  loadOriginal = soundPanelOriginalAudio,
}: SoundViewProps) {
  const { t } = useTranslation();
  const player = useSoundPlayer(loadMapAudio, loadOriginal);
  const rowRefs = useRef(new Map<string, HTMLLIElement>());
  const [highlight, setHighlight] = useState<string | null>(null);

  useEffect(() => {
    if (focus === null || list === null) return;
    const row = rowRefs.current.get(focus.mpqPath);
    row?.scrollIntoView?.({ block: "center" });
    setHighlight(focus.mpqPath);
  }, [focus, list]);

  const playButton = (track: Track, format: string, label: string) => {
    const key = trackKey(track);
    const isPlaying = player.playing === key;
    const isPending = player.pending === key;
    return (
      <div className="flex shrink-0 flex-col items-end gap-1">
        <Button
          type="button"
          size="sm"
          variant={isPlaying ? "secondary" : "outline"}
          aria-pressed={isPlaying}
          aria-label={isPlaying ? t("sound.stopLabel", { what: label }) : t("sound.playLabel", { what: label })}
          disabled={isPending}
          onClick={() => (isPlaying ? player.stop() : void player.play(track, format))}
        >
          {isPending ? (
            <Spinner className="size-3.5" />
          ) : isPlaying ? (
            <Square className="size-3.5" aria-hidden="true" />
          ) : (
            <Play className="size-3.5" aria-hidden="true" />
          )}
          {label}
          {isPlaying && (
            <span className="tabular-nums text-muted-foreground">{formatDuration(player.elapsedMs)}</span>
          )}
        </Button>
        {player.error?.key === key && (
          <p role="alert" className="max-w-56 text-right text-[11px] text-destructive">
            {t("sound.playFailed", { error: player.error.message })}
          </p>
        )}
      </div>
    );
  };

  const factsLine = (facts: SoundAudioFacts) =>
    [
      facts.codec,
      formatDuration(facts.durationMs),
      formatSignal(facts.sampleRate, facts.channels),
      formatBytes(facts.bytes),
    ]
      .filter(Boolean)
      .join(" · ");

  const renderSound = (sound: SoundPanelEntry) => {
    const importedIsOriginal =
      sound.importedAs !== null &&
      sound.originals.length === 1 &&
      sound.originals[0].sha256 === sound.importedAs.sha256;
    const effects =
      sound.volumePercent !== null &&
      (sound.volumePercent !== 100 || (sound.fadeInMs ?? 0) > 0 || (sound.fadeOutMs ?? 0) > 0);
    return (
      <li
        key={sound.mpqPath}
        ref={(node) => {
          if (node) rowRefs.current.set(sound.mpqPath, node);
          else rowRefs.current.delete(sound.mpqPath);
        }}
        className={cn(
          "flex flex-col gap-3 border-b border-border/60 px-4 py-3 transition-colors",
          highlight === sound.mpqPath && "bg-muted/50",
        )}
      >
        <div className="flex items-start gap-3">
          <span className="w-8 shrink-0 pt-0.5 text-right text-xs tabular-nums text-muted-foreground">
            #{sound.soundIndex}
          </span>
          <div className="min-w-0 flex-1">
            <p className="truncate text-sm font-medium" title={sound.mpqPath}>
              {sound.name}
            </p>
            <p className="truncate text-[11px] text-muted-foreground">{sound.mpqPath}</p>
          </div>
          {sound.originals.length === 0 && (
            <Badge variant="outline" className="shrink-0 text-[10px]">
              {t("sound.noOriginal")}
            </Badge>
          )}
        </div>

        <div className="ml-11 flex flex-col gap-2">
          <div className="flex items-center gap-3">
            <div className="min-w-0 flex-1 text-xs">
              <p className="font-medium">{t("sound.inMap")}</p>
              <p className="tabular-nums text-muted-foreground">
                {sound.inMap
                  ? [
                      sound.inMap.format,
                      formatDuration(sound.inMap.durationMs),
                      formatSignal(sound.inMap.sampleRate, sound.inMap.channels),
                      formatBytes(sound.inMap.bytes),
                    ]
                      .filter(Boolean)
                      .join(" · ")
                  : t("sound.notInMap")}
              </p>
              {effects && (
                <p className="text-muted-foreground">
                  {t("sound.effects", {
                    volume: sound.volumePercent,
                    fadeIn: formatDuration(sound.fadeInMs),
                    fadeOut: formatDuration(sound.fadeOutMs),
                  })}
                </p>
              )}
            </div>
            {sound.inMap &&
              playButton({ kind: "map", mpqPath: sound.mpqPath }, sound.inMap.format, t("sound.playInMap"))}
          </div>

          {sound.importedAs !== null && !importedIsOriginal && (
            <div className="text-xs">
              <p className="font-medium">{t("sound.importedAs")}</p>
              <p className="tabular-nums text-muted-foreground">
                {sound.importedAs.name} · {factsLine(sound.importedAs)}
              </p>
            </div>
          )}

          {sound.originals.map((original) => (
            <div key={original.sha256} className="flex items-center gap-3">
              <div className="min-w-0 flex-1 text-xs">
                <p className="font-medium">{t("sound.original")}</p>
                <p className="truncate tabular-nums text-muted-foreground" title={original.name}>
                  {original.name} · {factsLine(original)}
                </p>
              </div>
              {playButton({ kind: "original", sha256: original.sha256 }, original.codec, t("sound.playOriginal"))}
            </div>
          ))}
        </div>
      </li>
    );
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex items-center gap-3 border-b border-border px-4 py-3">
        <Music className="size-5 shrink-0 text-muted-foreground" aria-hidden="true" />
        <div className="min-w-0 flex-1">
          <h2 className="truncate text-sm font-semibold">{list?.mapName ?? t("sound.title")}</h2>
          {list !== null && (
            <p className="text-xs text-muted-foreground">
              {t("sound.summary", {
                count: list.sounds.length + list.omitted,
                size: formatBytes(list.totalBytes),
              })}
            </p>
          )}
        </div>
        <Button type="button" size="sm" variant="outline" disabled={loading} onClick={onRefresh}>
          {loading ? <Spinner className="size-3.5" /> : <RefreshCw className="size-3.5" aria-hidden="true" />}
          {t("sound.refresh")}
        </Button>
      </header>

      {error !== null && (
        <p role="alert" className="border-b border-border/60 px-4 py-3 text-xs text-destructive">
          {error}
        </p>
      )}
      {list === null && loading && (
        <div className="flex items-center gap-2 p-4 text-xs text-muted-foreground">
          <Spinner className="size-4" /> {t("sound.loading")}
        </div>
      )}
      {list !== null && list.sounds.length === 0 && (
        <p className="p-4 text-sm text-muted-foreground">{t("sound.empty")}</p>
      )}
      {list !== null && list.sounds.length > 0 && (
        <ul className="min-h-0 flex-1 overflow-y-auto">{list.sounds.map(renderSound)}</ul>
      )}
      {list !== null && list.omitted > 0 && (
        <p className="border-t border-border/60 px-4 py-2 text-xs text-muted-foreground">
          {t("sound.omitted", { count: list.omitted })}
        </p>
      )}
    </div>
  );
}
