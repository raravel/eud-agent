import { mkdir, readFile, rename, unlink, writeFile } from "node:fs/promises";
import { dirname } from "node:path";
import type { CorpusRow } from "./mapper.js";

export type CorpusJsonRow = CorpusRow & Record<string, unknown>;

const preferredKeyOrder = ["title", "content", "url", "source"];

export async function readCorpusRows(path: string): Promise<CorpusJsonRow[]> {
  let text: string;
  try {
    text = await readFile(path, "utf8");
  } catch (error) {
    if (isNodeError(error) && error.code === "ENOENT") {
      return [];
    }
    throw error;
  }

  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as CorpusJsonRow);
}

export async function writeCorpusJsonlAtomic(
  targetPath: string,
  rows: CorpusJsonRow[]
): Promise<void> {
  const tmpPath = `${targetPath}.tmp`;
  const text =
    rows.map((row) => serializeCorpusRow(row)).join("\n") +
    (rows.length > 0 ? "\n" : "");

  await mkdir(dirname(targetPath), { recursive: true });

  try {
    await writeFile(tmpPath, text, "utf8");
    await rename(tmpPath, targetPath);
  } catch (error) {
    await unlink(tmpPath).catch((unlinkError: unknown) => {
      if (!isNodeError(unlinkError) || unlinkError.code !== "ENOENT") {
        throw unlinkError;
      }
    });
    throw error;
  }
}

export function serializeCorpusRow(row: CorpusJsonRow): string {
  const ordered: Record<string, unknown> = {};

  for (const key of preferredKeyOrder) {
    if (key in row && row[key] !== undefined) {
      ordered[key] = row[key];
    }
  }

  for (const key of Object.keys(row).sort()) {
    if (!(key in ordered) && row[key] !== undefined) {
      ordered[key] = row[key];
    }
  }

  return JSON.stringify(ordered);
}

function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error;
}
