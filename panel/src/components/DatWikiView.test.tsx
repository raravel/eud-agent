import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type {
  DatWikiGraphic,
  DatWikiObjectValues,
  DatWikiSchema,
  DatWikiSheet,
  DatWikiSheetImage,
} from "@/lib/ipc";

import { resetDatWikiSheets } from "./DatWikiSheetFrame";
import { DatWikiView } from "./DatWikiView";

const datWikiObject = vi.fn<(table: string, objectId: number) => Promise<DatWikiObjectValues>>();
const datWikiSheet = vi.fn<(sheet: DatWikiSheet) => Promise<DatWikiSheetImage>>();
const datWikiGraphic =
  vi.fn<(table: string, objectId: number, slot?: string) => Promise<DatWikiGraphic>>();

vi.mock("@/lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/ipc")>()),
  datWikiObject: (table: string, objectId: number) => datWikiObject(table, objectId),
  datWikiSheet: (sheet: DatWikiSheet) => datWikiSheet(sheet),
  datWikiGraphic: (table: string, objectId: number, slot?: string) =>
    datWikiGraphic(table, objectId, slot),
}));

/**
 * Which cell of the grid an element is showing, derived from its own box so
 * the assertion does not depend on how large the view chose to draw it.
 */
function cell(element: HTMLElement): { column: number; row: number } {
  const { width, height, backgroundPosition } = element.style;
  const [x, y] = backgroundPosition.split(" ").map(Number.parseFloat);
  return {
    // `|| 0` normalises the -0 a negated zero offset produces.
    column: Math.round(-x / Number.parseFloat(width)) || 0,
    row: Math.round(-y / Number.parseFloat(height)) || 0,
  };
}

/**
 * A 4-frame grid: enough geometry for the slicing to be asserted on. A graphic
 * table's thumbnail sheet holds one cell per object, so `flingy` is larger.
 */
function sheetImage(sheet: DatWikiSheet): DatWikiSheetImage {
  return {
    sheet,
    png: `data:image/png;base64,${sheet}`,
    frameWidth: 36,
    frameHeight: 34,
    columns: 2,
    frames: sheet === "flingy" ? 80 : 4,
  };
}

/** The Marine's graphic as `dat_wiki_graphic` returns it for `slot`. */
function marineGraphic(slot = "Init"): DatWikiGraphic {
  return {
    png: `data:image/png;base64,marine-${slot}`,
    frameWidth: 64,
    frameHeight: 64,
    columns: 2,
    frames: 2,
    grp: "terran\\marine.grp",
    imageId: 239,
    iscriptId: 21,
    slots: ["Init", "Death", "Walking"],
    slot,
    steps: [
      { cell: 0, ticks: 1, flip: false },
      { cell: 1, ticks: 1, flip: false },
    ],
    loopStart: 0,
  };
}

/** A catalog small enough to assert on, with every value shape the view renders. */
const SCHEMA: DatWikiSchema = {
  pictures: true,
  tables: [
    {
      id: "units",
      kind: "dat",
      label: "유닛 (units)",
      graphic: true,
      objects: [
        { id: 0, name: "Terran Marine", picture: { sheet: "cmdicons", frame: 0 } },
        { id: 1, name: "Terran Ghost", picture: { sheet: "cmdicons", frame: 1 } },
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
      graphic: true,
      objects: [{ id: 78, name: "marine", picture: { sheet: "flingy", frame: 78 } }],
      fields: [],
      notice: "스타크래프트 설치 폴더를 찾지 못했습니다.",
    },
    {
      id: "wireframe",
      kind: "xdat",
      label: "와이어프레임 (wireframe)",
      graphic: false,
      objects: [{ id: 0, name: "Terran Marine", picture: { sheet: "wirefram", frame: 0 } }],
      fields: [
        {
          name: "wire",
          varStart: 0,
          varEnd: 0,
          size: 0,
          min: 0,
          max: 227,
          offset: 0,
          sheet: "wirefram",
        },
      ],
    },
    {
      id: "tbl",
      kind: "tbl",
      label: "문자열 (stat_txt)",
      graphic: false,
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
    resetDatWikiSheets();
    datWikiObject.mockReset();
    datWikiObject.mockResolvedValue(MARINE);
    datWikiSheet.mockReset();
    datWikiSheet.mockImplementation((sheet) => Promise.resolve(sheetImage(sheet)));
    datWikiGraphic.mockReset();
    datWikiGraphic.mockImplementation((_table, _objectId, slot) =>
      Promise.resolve(marineGraphic(slot)),
    );
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

  it("draws each row's icon out of one cached sheet, not one request per row", async () => {
    renderView();

    // Both units and the header draw frames, and the sheet is fetched once.
    const marine = await screen.findAllByRole("img", { name: "Terran Marine" });
    expect(marine.length).toBeGreaterThan(0);
    expect(screen.getByRole("img", { name: "Terran Ghost" })).toBeInTheDocument();
    expect(datWikiSheet).toHaveBeenCalledTimes(1);
    expect(datWikiSheet).toHaveBeenCalledWith("cmdicons");

    // Frame 1 sits in the second column of the first row of the 2-wide grid.
    expect(cell(screen.getByRole("img", { name: "Terran Ghost" }))).toEqual({
      column: 1,
      row: 0,
    });
    expect(cell(marine[0])).toEqual({ column: 0, row: 0 });
  });

  it("draws the object's own graphic and names the GRP it came from", async () => {
    renderView();

    await waitFor(() => expect(datWikiGraphic).toHaveBeenCalledWith("units", 0, undefined));
    const graphic = await screen.findByRole("img", { name: "Terran Marine의 그래픽" });
    expect(graphic.style.backgroundImage).toContain("marine-Init");
    expect(screen.getByText("terran\\marine.grp")).toBeInTheDocument();
  });

  it("plays the animation slot chosen beside the graphic", async () => {
    renderView();

    const slots = await screen.findByRole("combobox", { name: "애니메이션" });
    expect(slots).toHaveTextContent("Init");
    await userEvent.click(slots);
    await userEvent.click(screen.getByRole("option", { name: "Walking" }));

    await waitFor(() => expect(datWikiGraphic).toHaveBeenCalledWith("units", 0, "Walking"));
    await waitFor(() =>
      expect(
        screen.getByRole("img", { name: "Terran Marine의 그래픽" }).style.backgroundImage,
      ).toContain("marine-Walking"),
    );

    // Another object starts from its own Init, not the slot chosen here.
    fireEvent.click(screen.getByRole("button", { name: /Terran Ghost/ }));
    await waitFor(() => expect(datWikiGraphic).toHaveBeenLastCalledWith("units", 1, undefined));
  });

  it("draws a flingy row's thumbnail out of the flingy sheet", async () => {
    render(
      <DatWikiView
        schema={SCHEMA}
        loading={false}
        error={null}
        onRetry={vi.fn()}
        focus={{ table: "flingy", objectId: 78, nonce: 1 }}
      />,
    );

    await waitFor(() => expect(datWikiSheet).toHaveBeenCalledWith("flingy"));
    const [row] = await screen.findAllByRole("img", { name: "marine" });
    // Frame 78 of a 2-wide grid is the first column of row 39.
    expect(cell(row)).toEqual({ column: 0, row: 39 });
  });

  it("draws a field whose value is a frame number beside that value", async () => {
    datWikiObject.mockResolvedValue({
      table: "wireframe",
      objectId: 0,
      values: [{ field: "wire", stock: 3 }],
    });
    render(
      <DatWikiView
        schema={SCHEMA}
        loading={false}
        error={null}
        onRetry={vi.fn()}
        focus={{ table: "wireframe", objectId: 0, nonce: 1 }}
      />,
    );

    await waitFor(() => expect(datWikiSheet).toHaveBeenCalledWith("wirefram"));
    // Frame 3 of a 2-wide grid is the second column of the second row.
    expect(cell(await screen.findByRole("img", { name: "wire 3" }))).toEqual({
      column: 1,
      row: 1,
    });
    // A wireframe row has no graphic chain, so none is asked for. (The view
    // opens on its first table before the focus lands, which is that table's
    // graphic, not this one's.)
    expect(datWikiGraphic).not.toHaveBeenCalledWith("wireframe", expect.anything());
  });

  it("stays text-only, with the reason and a way back, without a StarCraft install", async () => {
    const onRetry = vi.fn();
    render(
      <DatWikiView
        schema={{
          ...SCHEMA,
          pictures: false,
          picturesNotice: "스타크래프트 설치 폴더를 찾지 못해 그림을 표시할 수 없습니다.",
        }}
        loading={false}
        error={null}
        onRetry={onRetry}
        focus={null}
      />,
    );

    expect(
      await screen.findByText("스타크래프트 설치 폴더를 찾지 못해 그림을 표시할 수 없습니다."),
    ).toBeInTheDocument();
    expect(screen.queryByRole("img", { name: "Terran Marine" })).not.toBeInTheDocument();
    expect(datWikiSheet).not.toHaveBeenCalled();
    expect(datWikiGraphic).not.toHaveBeenCalled();

    // Setting the folder is the stated recovery, so the notice can act on it.
    fireEvent.click(screen.getByRole("button", { name: "다시 읽기" }));
    expect(onRetry).toHaveBeenCalled();
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
