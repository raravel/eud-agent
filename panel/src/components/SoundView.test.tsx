import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

import { SoundView } from "@/components/SoundView";
import { SoundNav } from "@/components/SoundNav";
import type { SoundPanelList } from "@/lib/ipc";

const song = {
  sha256: "a".repeat(64),
  name: "song.mp3",
  codec: "mp3",
  durationMs: 225_000,
  channels: 2,
  sampleRate: 44_100,
  bytes: 5_400_000,
};

const list: SoundPanelList = {
  stamp: "map|1|1",
  mapName: "source.scx",
  totalBytes: 300_000,
  omitted: 0,
  sounds: [
    {
      soundIndex: 0,
      mpqPath: "staredit\\wav\\ea_1111111111111111.ogg",
      name: "ea_1111111111111111.ogg",
      managed: true,
      inMap: { bytes: 200_000, format: "ogg vorbis", durationMs: 12_300, channels: 2, sampleRate: 44_100 },
      importedAs: { ...song, sha256: "b".repeat(64), name: "intro.flac", codec: "flac", durationMs: 12_300 },
      originals: [song],
      volumePercent: 100,
      fadeInMs: 0,
      fadeOutMs: 0,
    },
    {
      soundIndex: 1,
      mpqPath: "staredit\\wav\\boom.wav",
      name: "boom.wav",
      managed: false,
      inMap: { bytes: 100_000, format: "wav", durationMs: 800, channels: 1, sampleRate: 22_050 },
      importedAs: null,
      originals: [],
      volumePercent: null,
      fadeInMs: null,
      fadeOutMs: null,
    },
  ],
};

describe("SoundView", () => {
  beforeEach(() => {
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => undefined);
    URL.createObjectURL = vi.fn(() => "blob:sound");
    URL.revokeObjectURL = vi.fn();
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("shows the cut, its uncut original, and sounds without one", () => {
    render(
      <SoundView list={list} loading={false} error={null} onRefresh={() => undefined} focus={null} />,
    );
    expect(screen.getByText(/intro\.flac/)).toBeTruthy();
    expect(screen.getByText(/song\.mp3 · mp3 · 3:45\.0/)).toBeTruthy();
    expect(screen.getByText(/ogg vorbis · 0:12\.3/)).toBeTruthy();
    // boom.wav has no original: one badge, one in-map player, no original player.
    expect(screen.getAllByText("원본 없음")).toHaveLength(1);
    expect(screen.getAllByRole("button", { name: "맵 안 재생" })).toHaveLength(2);
    expect(screen.getAllByRole("button", { name: "원본 재생" })).toHaveLength(1);
  });

  it("plays the original and the in-map file, one at a time", async () => {
    const loadOriginal = vi.fn(async () => new Uint8Array([1, 2, 3]));
    const loadMapAudio = vi.fn(async () => new Uint8Array([4, 5]));
    render(
      <SoundView
        list={list}
        loading={false}
        error={null}
        onRefresh={() => undefined}
        focus={null}
        loadOriginal={loadOriginal}
        loadMapAudio={loadMapAudio}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "원본 재생" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "원본 정지" }).getAttribute("aria-pressed")).toBe("true"),
    );
    expect(loadOriginal).toHaveBeenCalledWith(song.sha256);

    fireEvent.click(screen.getAllByRole("button", { name: "맵 안 재생" })[1]);
    await waitFor(() => expect(loadMapAudio).toHaveBeenCalledWith("staredit\\wav\\boom.wav"));
    await waitFor(() => expect(screen.queryByRole("button", { name: "원본 정지" })).toBeNull());
    expect(screen.getByRole("button", { name: "맵 안 정지" })).toBeTruthy();
  });

  it("reports a sound that cannot be read", async () => {
    render(
      <SoundView
        list={list}
        loading={false}
        error={null}
        onRefresh={() => undefined}
        focus={null}
        loadMapAudio={async () => {
          throw new Error("asset changed");
        }}
      />,
    );
    fireEvent.click(screen.getAllByRole("button", { name: "맵 안 재생" })[0]);
    expect(await screen.findByRole("alert")).toBeTruthy();
    expect(screen.getByRole("alert").textContent).toContain("asset changed");
  });
});

describe("SoundNav", () => {
  afterEach(cleanup);

  it("summarises the map and opens a sound", () => {
    const onOpen = vi.fn();
    render(
      <SoundNav list={list} loading={false} error={null} onRefresh={() => undefined} onOpen={onOpen} />,
    );
    expect(screen.getByText(/사운드 2개/)).toBeTruthy();
    fireEvent.click(screen.getByText("song.mp3"));
    expect(onOpen).toHaveBeenCalledWith("staredit\\wav\\ea_1111111111111111.ogg");
    expect(screen.getByText("boom.wav")).toBeTruthy();
  });
});
