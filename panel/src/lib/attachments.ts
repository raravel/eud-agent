import { invoke } from "@tauri-apps/api/core";
import type { AttachmentDescriptor, ChatAttachment } from "@/lib/protocol";

export const MAX_ATTACHMENTS_PER_TURN = 5;
export const MAX_IMAGE_BYTES = 10 * 1024 * 1024;
export const MAX_TEXT_BYTES = 512 * 1024;
export const MAX_AUDIO_BYTES = 64 * 1024 * 1024;
export const MAX_AUDIO_BYTES_PER_TURN = 128 * 1024 * 1024;

const IMAGE_EXTENSIONS = /\.(?:png|jpe?g|webp|gif)$/i;
const AUDIO_EXTENSIONS = /\.(?:wav|ogg|mp3|flac|m4a|aac|wma|aiff?|opus)$/i;
const FILE_NAME_HEADER = "x-eud-file-name-hex";
const THUMBNAIL_EDGE = 160;

type RawInvoke = typeof invoke;

export async function stageAttachment(
  file: File,
  invokeFn: RawInvoke = invoke,
): Promise<ChatAttachment> {
  const image = isLikelyImage(file);
  const audio = isLikelyAudio(file);
  const limit = image ? MAX_IMAGE_BYTES : audio ? MAX_AUDIO_BYTES : MAX_TEXT_BYTES;
  if (file.size === 0) {
    throw new Error(`빈 첨부 파일은 사용할 수 없습니다: ${file.name}`);
  }
  if (file.size > limit) {
    throw new Error(
      image
        ? `이미지 파일은 10MB 이하여야 합니다: ${file.name}`
        : audio
          ? `오디오 파일은 64MB 이하여야 합니다: ${file.name}`
          : `텍스트/코드 파일은 512KB 이하여야 합니다: ${file.name}`,
    );
  }

  const previewPromise = image
    ? createImageThumbnail(file).catch(() => undefined)
    : Promise.resolve(undefined);
  const bytes = new Uint8Array(await file.arrayBuffer());
  const descriptor = await invokeFn<AttachmentDescriptor>(
    "attachment_stage",
    bytes,
    {
      headers: {
        [FILE_NAME_HEADER]: encodeHeaderText(file.name),
        "content-type": safeMime(file.type),
      },
    },
  );
  const previewUrl = await previewPromise;
  return previewUrl === undefined ? descriptor : { ...descriptor, previewUrl };
}

export async function discardAttachment(
  id: string,
  invokeFn: RawInvoke = invoke,
): Promise<void> {
  await invokeFn("attachment_discard", { id });
}

export function attachmentErrorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

export function formatAttachmentSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.ceil(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function isLikelyImage(file: File): boolean {
  return file.type.startsWith("image/") || IMAGE_EXTENSIONS.test(file.name);
}

export function isLikelyAudio(file: File): boolean {
  return file.type.startsWith("audio/") || AUDIO_EXTENSIONS.test(file.name);
}

function safeMime(mime: string): string {
  return mime.length > 0 && mime.length <= 127 && /^[\x20-\x7e]+$/.test(mime)
    ? mime
    : "application/octet-stream";
}

function encodeHeaderText(value: string): string {
  let encoded = "";
  for (const byte of new TextEncoder().encode(value)) {
    encoded += byte.toString(16).padStart(2, "0");
  }
  return encoded;
}

async function createImageThumbnail(file: File): Promise<string | undefined> {
  if (
    typeof document === "undefined" ||
    typeof createImageBitmap !== "function"
  ) {
    return undefined;
  }
  const image = await createImageBitmap(file);
  try {
    const scale = Math.min(
      1,
      THUMBNAIL_EDGE / Math.max(image.width, image.height),
    );
    const width = Math.max(1, Math.round(image.width * scale));
    const height = Math.max(1, Math.round(image.height * scale));
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext("2d");
    if (context === null) return undefined;
    context.drawImage(image, 0, 0, width, height);
    return canvas.toDataURL("image/webp", 0.72);
  } finally {
    image.close();
  }
}
