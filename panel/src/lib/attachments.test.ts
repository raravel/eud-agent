import { describe, expect, it, vi } from "vitest";
import {
  isLikelyAudio,
  MAX_AUDIO_BYTES,
  stageAttachment,
} from "@/lib/attachments";

function sizedFile(name: string, type: string, size: number): File {
  const file = new File(["x"], name, { type });
  Object.defineProperty(file, "size", { value: size });
  return file;
}

describe("audio attachment staging", () => {
  it("recognizes common audio picker hints without treating arbitrary binary as audio", () => {
    for (const name of [
      "a.wav",
      "a.ogg",
      "a.mp3",
      "a.flac",
      "a.m4a",
      "a.aac",
      "a.wma",
      "a.aiff",
      "a.aif",
      "a.opus",
    ]) {
      expect(isLikelyAudio(new File(["x"], name))).toBe(true);
    }
    expect(isLikelyAudio(new File(["x"], "clip.bin"))).toBe(false);
    expect(isLikelyAudio(new File(["x"], "opaque", { type: "audio/ogg" }))).toBe(
      true,
    );
  });

  it("enforces zero and 64 MiB client bounds before IPC", async () => {
    const invoke = vi.fn();
    await expect(
      stageAttachment(sizedFile("empty.wav", "audio/wav", 0), invoke),
    ).rejects.toThrow("빈 첨부 파일");
    await expect(
      stageAttachment(
        sizedFile("large.flac", "audio/flac", MAX_AUDIO_BYTES + 1),
        invoke,
      ),
    ).rejects.toThrow("64MB");
    expect(invoke).not.toHaveBeenCalled();
  });

  it("keeps audio metadata only and never creates a preview data URL", async () => {
    const descriptor = {
      id: "audio-1",
      name: "effect.ogg",
      mime: "audio/ogg",
      kind: "audio" as const,
      size: 4,
    };
    const invoke = vi.fn().mockResolvedValue(descriptor);
    const staged = await stageAttachment(
      new File(["OggS"], "effect.ogg", { type: "audio/ogg" }),
      invoke,
    );
    expect(staged).toEqual(descriptor);
    expect(staged).not.toHaveProperty("previewUrl");
    expect(invoke).toHaveBeenCalledWith(
      "attachment_stage",
      expect.any(Uint8Array),
      expect.objectContaining({
        headers: expect.objectContaining({ "content-type": "audio/ogg" }),
      }),
    );
  });
});
