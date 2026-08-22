import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { QualifierEditor } from "./QualifierEditor";
import type { MentionChip, SavedSelection } from "./mapProtocol";

const chip: MentionChip = {
  id: "chip",
  label: "type:Terran Bunker",
  mention: {
    kind: "palette",
    entry: {
      layer: "buildings",
      kind: "building",
      entryId: 125,
      tileset: "jungle",
      fingerprint: "bunker",
    },
    qualifiers: {},
  },
};

describe("Map mention qualifier editor", () => {
  it("preserves structured owner, count, state instead of rewriting display text", async () => {
    const onChange = vi.fn();
    render(<QualifierEditor chip={chip} onChange={onChange} />);
    await userEvent.type(screen.getByLabelText("Owner (0–11)"), "4");
    expect(onChange).toHaveBeenLastCalledWith({ owner: 4 });
    await userEvent.click(screen.getByRole("checkbox", { name: "Invincible" }));
    expect(onChange).toHaveBeenLastCalledWith({ invincible: true });
  });

  it("stores a new location name with an exact saved-selection snapshot relation", async () => {
    const locationChip: MentionChip = {
      id: "location",
      label: "type:새 로케이션",
      mention: {
        kind: "palette",
        entry: {
          layer: "locations",
          kind: "newLocation",
          entryId: 0,
          tileset: "jungle",
          fingerprint: "new-location/1",
        },
        qualifiers: {},
      },
    };
    const selection = {
      id: "selection-a",
      label: "영역 A",
      snapshotHash: "snapshot-a",
      sourceRevision: "r1:hash",
      selectedCells: 12,
    } as SavedSelection;
    const onChange = vi.fn();
    const { rerender } = render(
      <QualifierEditor
        chip={locationChip}
        selections={[selection]}
        mapWidth={64}
        mapHeight={64}
        onChange={onChange}
      />,
    );
    await userEvent.click(screen.getByLabelText("Bounds source"));
    await userEvent.click(screen.getByRole("option", { name: /영역 A/ }));
    const selectionQualifiers = onChange.mock.lastCall?.[0];
    expect(selectionQualifiers).toEqual({
      locationSelection: {
        selectionId: "selection-a",
        snapshotHash: "snapshot-a",
        sourceRevision: "r1:hash",
      },
    });
    rerender(
      <QualifierEditor
        chip={{
          ...locationChip,
          mention: {
            ...locationChip.mention,
            qualifiers: selectionQualifiers,
          },
        }}
        selections={[selection]}
        mapWidth={64}
        mapHeight={64}
        onChange={onChange}
      />,
    );
    await userEvent.type(screen.getByLabelText("로케이션 이름"), "공격지점");
    expect(onChange).toHaveBeenLastCalledWith({
      ...selectionQualifiers,
      locationName: "점",
    });
  });
});
