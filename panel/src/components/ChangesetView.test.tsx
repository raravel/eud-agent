/**
 * Changeset review (features/06 ## UI layout + Behaviors). Renders
 * `changeset.items[]` grouped (the server groups dat per objId, files by kind,
 * the rest flat). Per-item [✓ accept]/[✗ reject] + bulk [전체 적용 유지]/
 * [전체 되돌리기] map to the store's `changeset_decision{decision, ids}` (single
 * ids vs "all"). `rollback_result` row states surface via the store decisions:
 * accepted (적용 유지) / 되돌림 / 실패 (inline failure). Korean labels.
 *
 * Contract (Step B implements `@/components/ChangesetView`):
 *   export interface ChangesetViewProps {
 *     changeset: ChangesetState;          // store.changeset (items + decisions)
 *     pending: boolean;                    // a decision is in flight (disable)
 *     onDecide(decision: "accept" | "reject", ids: "all" | string[]): void;
 *   }
 *   export function ChangesetView(props): JSX.Element;
 *
 * onDecide receives the SAME (decision, ids) the store's decisionSent() records,
 * and the App layer invokes `changeset_decision` from it.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ChangesetView } from "@/components/ChangesetView";
import type { ChangesetState } from "@/state/store";
import type { ChangesetItem } from "@/lib/ipc";

// REPRESENTATIVE: a server-assembled dat group carries NO item-level id/seq —
// the ids live only on properties[] (journal.changeset()). The ChangesetItem
// type requires id/seq, but production dat groups omit them, so the fixture
// casts to mirror the real shape. itemKey derives a stable key from the joined
// property ids ("p1,p2"), used for both React key and data-testid.
const datItem = {
  category: "dat",
  dat: "unit",
  objId: 76,
  name: "마린",
  properties: [
    { property: "HP", old: "40", new: "80", id: "p1", seq: 0 },
    { property: "Gas", old: "0", new: "25", id: "p2", seq: 1 },
  ],
} as unknown as ChangesetItem;
/** itemKey(datItem) — the joined property ids (no item-level id). */
const DAT_KEY = "p1,p2";

// A SECOND dat group (a different edited unit) — also id-less. Its properties
// carry distinct ids so the two groups render with distinct testids and decide
// independently (the multi-dat-group regression: undefined-keyed collisions).
const datItem2 = {
  category: "dat",
  dat: "unit",
  objId: 0,
  name: "테란 마린",
  properties: [
    { property: "HP", old: "10", new: "20", id: "q1", seq: 4 },
  ],
} as unknown as ChangesetItem;
const DAT_KEY2 = "q1";

const createdFile: ChangesetItem = {
  category: "file",
  kind: "created",
  path: "teleport.eps",
  content: "function tp() {}",
  id: "fc",
  seq: 0,
};

const modifiedFile: ChangesetItem = {
  category: "file",
  kind: "modified",
  path: "main.eps",
  id: "fm",
  seq: 1,
  diff: "--- a/main.eps\n+++ b/main.eps\n@@ -1 +1 @@\n-old line\n+new line\n",
};

const deletedFile: ChangesetItem = {
  category: "file",
  kind: "deleted",
  path: "old.eps",
  id: "fd",
  seq: 3,
};

const settingsItem: ChangesetItem = {
  category: "settings",
  tool: "settings_set",
  target: "trigger_editor",
  old: { value: "off" },
  new: { value: "on" },
  id: "s1",
  seq: 4,
};

function makeChangeset(
  items: ChangesetItem[],
  decisions: ChangesetState["decisions"] = {},
): ChangesetState {
  return { request_id: "req-1", items, decisions };
}

describe("ChangesetView — dat grouping", () => {
  it("renders a dat group header with dat, objId and (server-resolved) name", () => {
    render(
      <ChangesetView
        changeset={makeChangeset([datItem])}
        pending={false}
        onDecide={() => {}}
      />,
    );
    // e.g. "unit [76] 마린" — assert all three pieces are present.
    expect(screen.getByText(/unit/)).toBeInTheDocument();
    expect(screen.getByText(/76/)).toBeInTheDocument();
    expect(screen.getByText(/마린/)).toBeInTheDocument();
  });

  it("renders each property as an old → new row", () => {
    render(
      <ChangesetView
        changeset={makeChangeset([datItem])}
        pending={false}
        onDecide={() => {}}
      />,
    );
    expect(screen.getByText(/HP/)).toBeInTheDocument();
    expect(screen.getByText(/40/)).toBeInTheDocument();
    expect(screen.getByText(/80/)).toBeInTheDocument();
    expect(screen.getByText(/Gas/)).toBeInTheDocument();
    expect(screen.getByText(/25/)).toBeInTheDocument();
  });
});

describe("ChangesetView — files by kind", () => {
  it("renders a created file with a content preview", () => {
    render(
      <ChangesetView
        changeset={makeChangeset([createdFile])}
        pending={false}
        onDecide={() => {}}
      />,
    );
    expect(screen.getByText(/teleport\.eps/)).toBeInTheDocument();
    expect(screen.getByText(/function tp/)).toBeInTheDocument();
  });

  it("renders a modified file's unified diff with +/- line coloring (no Monaco)", () => {
    const { container } = render(
      <ChangesetView
        changeset={makeChangeset([modifiedFile])}
        pending={false}
        onDecide={() => {}}
      />,
    );
    // The path appears in the header AND in the diff file-header lines; assert
    // it renders at least once.
    expect(screen.getAllByText(/main\.eps/).length).toBeGreaterThan(0);
    // diff lines classified: at least one add + one del line element.
    expect(container.querySelector('[data-diff="add"]')).not.toBeNull();
    expect(container.querySelector('[data-diff="del"]')).not.toBeNull();
  });

  it("renders a deleted file as a name row", () => {
    render(
      <ChangesetView
        changeset={makeChangeset([deletedFile])}
        pending={false}
        onDecide={() => {}}
      />,
    );
    expect(screen.getByText(/old\.eps/)).toBeInTheDocument();
  });
});

describe("ChangesetView — decision dispatch (single vs all)", () => {
  it("a per-item accept dispatches the item's ids (file: single id)", async () => {
    const onDecide = vi.fn();
    render(
      <ChangesetView
        changeset={makeChangeset([modifiedFile])}
        pending={false}
        onDecide={onDecide}
      />,
    );
    const row = screen.getByTestId("cs-item-fm");
    await userEvent.click(within(row).getByRole("button", { name: /적용/ }));
    expect(onDecide).toHaveBeenCalledWith("accept", ["fm"]);
  });

  it("a per-item reject dispatches the item's ids", async () => {
    const onDecide = vi.fn();
    render(
      <ChangesetView
        changeset={makeChangeset([modifiedFile])}
        pending={false}
        onDecide={onDecide}
      />,
    );
    const row = screen.getByTestId("cs-item-fm");
    await userEvent.click(within(row).getByRole("button", { name: /되돌/ }));
    expect(onDecide).toHaveBeenCalledWith("reject", ["fm"]);
  });

  it("a dat group (no item-level id) accept dispatches ALL its property ids", async () => {
    const onDecide = vi.fn();
    render(
      <ChangesetView
        changeset={makeChangeset([datItem])}
        pending={false}
        onDecide={onDecide}
      />,
    );
    const row = screen.getByTestId(`cs-item-${DAT_KEY}`);
    await userEvent.click(within(row).getByRole("button", { name: /적용/ }));
    expect(onDecide).toHaveBeenCalledWith("accept", ["p1", "p2"]);
  });

  it("renders MULTIPLE id-less dat groups with distinct testids that decide independently", async () => {
    const onDecide = vi.fn();
    render(
      <ChangesetView
        changeset={makeChangeset([datItem, datItem2])}
        pending={false}
        onDecide={onDecide}
      />,
    );
    // Distinct rows (no `cs-item-undefined` collision).
    const row1 = screen.getByTestId(`cs-item-${DAT_KEY}`);
    const row2 = screen.getByTestId(`cs-item-${DAT_KEY2}`);
    expect(row1).not.toBe(row2);
    // Each row's accept targets ONLY its own property ids.
    await userEvent.click(within(row1).getByRole("button", { name: /적용/ }));
    expect(onDecide).toHaveBeenLastCalledWith("accept", ["p1", "p2"]);
    await userEvent.click(within(row2).getByRole("button", { name: /적용/ }));
    expect(onDecide).toHaveBeenLastCalledWith("accept", ["q1"]);
  });

  it("bulk [전체 적용 유지] dispatches the literal \"all\"", async () => {
    const onDecide = vi.fn();
    render(
      <ChangesetView
        changeset={makeChangeset([datItem, modifiedFile])}
        pending={false}
        onDecide={onDecide}
      />,
    );
    await userEvent.click(screen.getByRole("button", { name: "전체 적용 유지" }));
    expect(onDecide).toHaveBeenCalledWith("accept", "all");
  });

  it("bulk [전체 되돌리기] dispatches reject with the literal \"all\"", async () => {
    const onDecide = vi.fn();
    render(
      <ChangesetView
        changeset={makeChangeset([datItem, modifiedFile])}
        pending={false}
        onDecide={onDecide}
      />,
    );
    await userEvent.click(screen.getByRole("button", { name: "전체 되돌리기" }));
    expect(onDecide).toHaveBeenCalledWith("reject", "all");
  });

  it("disables decision buttons while a decision is pending", () => {
    render(
      <ChangesetView
        changeset={makeChangeset([modifiedFile])}
        pending={true}
        onDecide={() => {}}
      />,
    );
    const row = screen.getByTestId("cs-item-fm");
    expect(within(row).getByRole("button", { name: /적용/ })).toBeDisabled();
  });

  // EUD-070: a rollback replays inverse ops over the 1s-tick file IPC (2-4s
  // for a dat group) — the in-flight wait needs a visible progress notice, not
  // just silently-disabled buttons (the live-E2E "동기적 렉" perception).
  it("shows an in-flight notice with a spinner while a decision is pending", () => {
    render(
      <ChangesetView
        changeset={makeChangeset([modifiedFile])}
        pending={true}
        onDecide={() => {}}
      />,
    );
    expect(screen.getByRole("status")).toBeInTheDocument();
    expect(screen.getByText(/처리 중/)).toBeInTheDocument();
  });

  it("hides the in-flight notice when no decision is pending", () => {
    render(
      <ChangesetView
        changeset={makeChangeset([modifiedFile])}
        pending={false}
        onDecide={() => {}}
      />,
    );
    expect(screen.queryByText(/처리 중/)).not.toBeInTheDocument();
  });
});

describe("ChangesetView — rollback_result row states", () => {
  it("shows 적용 유지 for an accepted item", () => {
    render(
      <ChangesetView
        changeset={makeChangeset([modifiedFile], { fm: "accepted" })}
        pending={false}
        onDecide={() => {}}
      />,
    );
    const row = screen.getByTestId("cs-item-fm");
    expect(within(row).getByText(/적용 유지/)).toBeInTheDocument();
  });

  it("shows 되돌림 for a rejected item", () => {
    render(
      <ChangesetView
        changeset={makeChangeset([modifiedFile], { fm: "rejected" })}
        pending={false}
        onDecide={() => {}}
      />,
    );
    const row = screen.getByTestId("cs-item-fm");
    expect(within(row).getByText(/되돌림/)).toBeInTheDocument();
  });

  it("surfaces a rollback failure inline (실패)", () => {
    render(
      <ChangesetView
        changeset={makeChangeset([modifiedFile], { fm: "failed" })}
        pending={false}
        onDecide={() => {}}
      />,
    );
    const row = screen.getByTestId("cs-item-fm");
    expect(within(row).getByText(/실패/)).toBeInTheDocument();
  });

  it("renders settings as an old → new row", () => {
    render(
      <ChangesetView
        changeset={makeChangeset([settingsItem])}
        pending={false}
        onDecide={() => {}}
      />,
    );
    expect(screen.getByText(/off/)).toBeInTheDocument();
    expect(screen.getByText(/on/)).toBeInTheDocument();
  });

  it("shows INDEPENDENT badges across two id-less dat groups", () => {
    // Group 1 (p1,p2) accepted; group 2 (q1) rejected — each row reflects only
    // its own properties' decisions (no cross-group bleed from undefined keys).
    render(
      <ChangesetView
        changeset={makeChangeset([datItem, datItem2], {
          p1: "accepted",
          p2: "accepted",
          q1: "rejected",
        })}
        pending={false}
        onDecide={() => {}}
      />,
    );
    const row1 = screen.getByTestId(`cs-item-${DAT_KEY}`);
    const row2 = screen.getByTestId(`cs-item-${DAT_KEY2}`);
    expect(within(row1).getByText(/적용 유지/)).toBeInTheDocument();
    expect(within(row2).getByText(/되돌림/)).toBeInTheDocument();
    // The accepted group is NOT mislabelled as rejected, and vice versa.
    expect(within(row1).queryByText(/되돌림/)).not.toBeInTheDocument();
    expect(within(row2).queryByText(/적용 유지/)).not.toBeInTheDocument();
  });
});
