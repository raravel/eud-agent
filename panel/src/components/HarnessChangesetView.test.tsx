import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { HarnessChangesetView } from "@/components/HarnessChangesetView";
import type { ChangesetItem } from "@/lib/ipc";

function property(name: string, old: unknown, next: unknown) {
  return { property: name, old, new: next, id: "sound-1", seq: 1 };
}

describe("HarnessChangesetView sound items", () => {
  it("shows a removed map sound as a removal with its old path and slot", () => {
    // Given: the changeset of one map_sound_remove journal entry.
    const removal = {
      id: "sound-1",
      category: "mapSound",
      kind: "mapSound",
      properties: [
        property("removed", null, true),
        property("source", null, "battle-theme.flac"),
        property("mpqPath", "staredit\\wav\\ea_0123456789abcdef.ogg", null),
        property("wavIndex", 12, null),
        property("mapSizeDelta", null, -2048),
      ],
    } as unknown as ChangesetItem;

    // When: the review renders it.
    render(
      <HarnessChangesetView
        items={[removal]}
        open
        onOpenChange={vi.fn()}
        pending={false}
        onDecide={vi.fn()}
      />,
    );

    // Then: it reads as a removal, never as an addition.
    const card = screen.getByLabelText("오디오 제거 battle-theme.flac");
    expect(card).toHaveTextContent("오디오 제거 · battle-theme.flac");
    expect(card).toHaveTextContent("staredit\\wav\\ea_0123456789abcdef.ogg");
    expect(card).toHaveTextContent("#12");
    expect(card).toHaveTextContent("−2");
    expect(screen.queryByText(/오디오 추가/)).toBeNull();
  });
});
