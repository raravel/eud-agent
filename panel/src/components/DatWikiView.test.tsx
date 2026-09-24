import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { DatWikiObjectValues, DatWikiSchema } from "@/lib/ipc";

import { DatWikiView } from "./DatWikiView";

const datWikiObject = vi.fn<(table: string, objectId: number) => Promise<DatWikiObjectValues>>();

vi.mock("@/lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/ipc")>()),
  datWikiObject: (table: string, objectId: number) => datWikiObject(table, objectId),
}));

/** A catalog small enough to assert on, with every value shape the view renders. */
const SCHEMA: DatWikiSchema = {
  tables: [
    {
      id: "units",
      kind: "dat",
      label: "유닛 (units)",
      objects: [
        { id: 0, name: "Terran Marine" },
        { id: 1, name: "Terran Ghost" },
      ],
      fields: [
        {
          name: "Graphics",
          varStart: 0,
          varEnd: 1,
          size: 1,
          min: 0,
          max: 255,
          offset: 0x6644f8,
          reference: { table: "flingy" },
        },
        {
          name: "Hit Points",
          varStart: 0,
          varEnd: 1,
          size: 4,
          min: 0,
          max: 4294967295,
          offset: 0x662350,
        },
        {
          name: "Rank/Sublabel",
          varStart: 0,
          varEnd: 1,
          size: 1,
          min: 1302,
          max: 1557,
          offset: 0x663dd0,
          reference: "text",
        },
        {
          name: "Special Ability Flags",
          varStart: 0,
          varEnd: 1,
          size: 4,
          min: 0,
          max: 4294967295,
          offset: 0x664080,
          flags: ["Building", "Addon", "Flyer", "Worker"],
        },
      ],
    },
    {
      id: "flingy",
      kind: "dat",
      label: "플링기 (flingy)",
      objects: [{ id: 78, name: "marine" }],
      fields: [],
      notice: "스타크래프트 설치 폴더를 찾지 못했습니다.",
    },
    {
      id: "tbl",
      kind: "tbl",
      label: "문자열 (stat_txt)",
      // Zero-based, so the one-based id 1304 is this list's index 1303.
      objects: Array.from({ length: 1304 }, (_, index) => ({
        id: index,
        name: index === 1303 ? "Private" : `문자열 ${index}`,
      })),
      fields: [{ name: "문자열", varStart: 0, varEnd: 1303, size: 0, min: 0, max: 0, offset: 0 }],
    },
  ],
};

const MARINE: DatWikiObjectValues = {
  table: "units",
  objectId: 0,
  values: [
    { field: "Graphics", stock: 78 },
    { field: "Hit Points", stock: 40, current: 80 },
    { field: "Rank/Sublabel", stock: 1304 },
    { field: "Special Ability Flags", stock: 0b0101 },
  ],
};

function renderView() {
  return render(
    <DatWikiView schema={SCHEMA} loading={false} error={null} onRetry={vi.fn()} focus={null} />,
  );
}

describe("DatWikiView", () => {
  beforeEach(() => {
    datWikiObject.mockReset();
    datWikiObject.mockResolvedValue(MARINE);
  });

  it("reads a reference field as the object it points at, and follows it", async () => {
    renderView();

    expect(await screen.findByRole("button", { name: "marine" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "marine" }));

    await waitFor(() => expect(datWikiObject).toHaveBeenCalledWith("flingy", 78));
    expect(
      screen.getByText("스타크래프트 설치 폴더를 찾지 못했습니다."),
    ).toBeInTheDocument();
  });

  it("reads a label field through the ONE-based string id", async () => {
    renderView();

    // 1304 is the one-based id; the zero-based `tbl` list holds it at 1303.
    expect(await screen.findByRole("button", { name: "“Private”" })).toBeInTheDocument();
  });

  it("shows one labelled bit per flag, set and unset", async () => {
    renderView();

    expect(await screen.findByTitle("Building")).toBeInTheDocument();
    const building = screen.getByTitle("Building").closest("li");
    const addon = screen.getByTitle("Addon").closest("li");
    const flyer = screen.getByTitle("Flyer").closest("li");
    expect(building).toHaveTextContent("켜짐");
    expect(addon).toHaveTextContent("꺼짐");
    expect(flyer).toHaveTextContent("켜짐");
  });

  it("shows an overridden field as the project's value beside the stock one", async () => {
    renderView();

    expect(await screen.findByText("수정됨")).toBeInTheDocument();
    expect(screen.getByText("80")).toBeInTheDocument();
    expect(screen.getByText("원본")).toBeInTheDocument();
    expect(screen.getByText("40")).toBeInTheDocument();
  });

  it("loads the object a focus request names", async () => {
    const { rerender } = renderView();
    await screen.findByText("수정됨");

    rerender(
      <DatWikiView
        schema={SCHEMA}
        loading={false}
        error={null}
        onRetry={vi.fn()}
        focus={{ table: "units", objectId: 1, nonce: 1 }}
      />,
    );

    await waitFor(() => expect(datWikiObject).toHaveBeenCalledWith("units", 1));
    expect(screen.getByRole("heading", { name: "Terran Ghost" })).toBeInTheDocument();
  });
});
