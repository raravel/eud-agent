import { useRef, useState } from "react";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import {
  MentionComposer,
  activeMentionFragment,
} from "@/components/MentionComposer";
import type {
  MentionInstance,
  MentionSearchRequest,
  MentionSearchResponse,
  MentionSuggestion,
} from "@/lib/ipc";
import type { ProjectFileSuggestion } from "@/lib/projectFiles";

const mainFile: ProjectFileSuggestion = {
  workspaceId: "workspace-a",
  path: "src/main.eps",
  size: 120,
};

const region: MentionSuggestion = {
  resourceKey: "map.region:region-a",
  kind: "map.region",
  label: "영역 A",
  detail: "저장된 영역 · 사각형",
  mention: {
    kind: "map.region",
    version: 1,
    projectId: "project-a",
    sourceFileSha256: "a".repeat(64),
    mapWidth: 64,
    mapHeight: 64,
    selectionId: "region-a",
    selectionSnapshotHash: "b".repeat(64),
  },
};

const location: MentionSuggestion = {
  resourceKey: "map.location:17",
  kind: "map.location",
  label: "회복 지점",
  detail: "저장된 소스 맵 · #17",
  mention: {
    kind: "map.location",
    version: 1,
    projectId: "project-a",
    sourceFileSha256: "a".repeat(64),
    locationId: 17,
    locationFingerprint: "c".repeat(64),
  },
};

function response(results: MentionSuggestion[]): MentionSearchResponse {
  return { schema: "eud-mention-search/1", results, truncated: false };
}

function Harness({
  search,
  fileSearch,
  onAttachFile,
  initialText = "",
  project = "project-a",
  scope = "session-a",
}: {
  search(request: MentionSearchRequest): Promise<MentionSearchResponse>;
  fileSearch?(query: string): Promise<ProjectFileSuggestion[]>;
  onAttachFile?(file: ProjectFileSuggestion): void;
  initialText?: string;
  project?: string;
  scope?: string;
}) {
  const [text, setText] = useState(initialText);
  const [mentions, setMentions] = useState<MentionInstance[]>([]);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  return (
    <MentionComposer
      text={text}
      onTextChange={setText}
      mentions={mentions}
      onMentionsChange={setMentions}
      search={search}
      fileSearch={fileSearch}
      onAttachFile={onAttachFile}
      projectIdentity={project}
      scopeIdentity={scope}
      textareaRef={textareaRef}
    />
  );
}

describe("MentionComposer", () => {
  it("detects @ at the caret and keeps Korean labels containing spaces", () => {
    expect(activeMentionFragment("앞 @영역 A 뒤", "앞 @영역 A".length)).toMatchObject({
      query: "영역 A",
      start: 2,
      end: "앞 @영역 A".length,
    });
    expect(activeMentionFragment("mail@example.com", 16)).toBeNull();
  });

  it("opens bounded caret search and removes only the active fragment", async () => {
    const search = vi.fn(async () => response([region]));
    render(<Harness search={search} />);
    const input = screen.getByRole("combobox", { name: "지시 입력" });
    fireEvent.change(input, { target: { value: "앞 @영역 A 뒤" } });
    input.setSelectionRange("앞 @영역 A".length, "앞 @영역 A".length);
    fireEvent.select(input);

    await screen.findByRole("option", { name: /@영역 A/ });
    expect(search).toHaveBeenLastCalledWith({ query: "영역 A", limit: 20 });
    fireEvent.click(screen.getByRole("option", { name: /@영역 A/ }));

    expect(input).toHaveValue("앞  뒤");
    expect(screen.getByTestId("mention-chips")).toHaveTextContent("@영역 A");
  });

  it("issues one backend search for one typed fragment", async () => {
    const search = vi.fn(async () => response([region]));
    render(<Harness search={search} />);
    const input = screen.getByRole("combobox", { name: "지시 입력" });

    await userEvent.type(input, "@");
    await screen.findByRole("option", { name: /@영역 A/ });

    expect(search).toHaveBeenCalledTimes(1);
    expect(search).toHaveBeenCalledWith({ query: "", limit: 20 });
  });

  it("renders loading, empty, and error states", async () => {
    let resolveSearch: ((value: MentionSearchResponse) => void) | undefined;
    const pending = new Promise<MentionSearchResponse>((resolve) => {
      resolveSearch = resolve;
    });
    const search = vi.fn(() => pending);
    const view = render(<Harness search={search} />);
    const input = screen.getByRole("combobox", { name: "지시 입력" });
    await userEvent.type(input, "@");
    expect(screen.getByRole("status")).toHaveTextContent("검색 중");
    await act(async () => resolveSearch?.(response([])));
    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveTextContent("검색 결과가 없습니다"),
    );

    view.unmount();
    render(
      <Harness search={vi.fn(async () => Promise.reject(new Error("offline")))} />,
    );
    await userEvent.type(screen.getByRole("combobox", { name: "지시 입력" }), "@");
    expect(await screen.findByRole("alert")).toHaveTextContent("offline");
  });

  it("supports Arrow navigation, Enter selection, Escape, and IME safety", async () => {
    const search = vi.fn(async () => response([region, location]));
    render(<Harness search={search} />);
    const input = screen.getByRole("combobox", { name: "지시 입력" });
    await userEvent.type(input, "@");
    await screen.findByRole("option", { name: /@영역 A/ });

    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(screen.getByRole("option", { name: /@회복 지점/ })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    fireEvent.keyDown(input, { key: "Enter" });
    expect(screen.getByTestId("mention-chips")).toHaveTextContent("@회복 지점");

    await userEvent.type(input, "@");
    await screen.findByRole("listbox");
    fireEvent.keyDown(input, { key: "Escape" });
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();

    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "@영역" } });
    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    expect(screen.getByTestId("mention-chips")).not.toHaveTextContent("@영역 A");
    fireEvent.compositionEnd(input);
    // The Enter that committed the syllable completes the committed query.
    await waitFor(() =>
      expect(screen.getByTestId("mention-chips")).toHaveTextContent("@영역 A"),
    );
    expect(input).toHaveValue("");
  });

  it("searches the query an IME is still composing", async () => {
    const search = vi.fn(async () => response([location]));
    render(<Harness search={search} />);
    const input = screen.getByRole("combobox", { name: "지시 입력" });

    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "@회복" } });

    await screen.findByRole("option", { name: /@회복 지점/ });
    expect(search).toHaveBeenLastCalledWith({ query: "회복", limit: 20 });
  });

  it("completes the active option with Tab, also while composing", async () => {
    const search = vi.fn(async ({ query }: MentionSearchRequest) =>
      response([region, location].filter((item) => item.label.includes(query))),
    );
    render(<Harness search={search} />);
    const input = screen.getByRole("combobox", { name: "지시 입력" });

    await userEvent.type(input, "@");
    await screen.findByRole("option", { name: /@영역 A/ });
    fireEvent.keyDown(input, { key: "Tab" });
    expect(screen.getByTestId("mention-chips")).toHaveTextContent("@영역 A");
    expect(input).toHaveFocus();

    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "@회복" } });
    fireEvent.keyDown(input, { key: "Tab", isComposing: true });
    fireEvent.compositionEnd(input);
    await waitFor(() =>
      expect(screen.getByTestId("mention-chips")).toHaveTextContent("@회복 지점"),
    );
  });

  it("offers project files and attaches the picked one instead of a chip", async () => {
    const search = vi.fn(async () => Promise.reject(new Error("no map")));
    const fileSearch = vi.fn(async () => [mainFile]);
    const onAttachFile = vi.fn();
    render(
      <Harness search={search} fileSearch={fileSearch} onAttachFile={onAttachFile} />,
    );
    const input = screen.getByRole("combobox", { name: "지시 입력" });

    await userEvent.type(input, "보기 @main");
    const option = await screen.findByRole("option", { name: /@main\.eps/ });
    expect(option).toHaveTextContent("src");
    expect(fileSearch).toHaveBeenLastCalledWith("main");
    fireEvent.keyDown(input, { key: "Tab" });

    expect(onAttachFile).toHaveBeenCalledWith(mainFile);
    expect(input).toHaveValue("보기 ");
    expect(screen.queryByTestId("mention-chips")).toBeNull();
  });

  it("preserves mixed order, prevents exact duplicates, and removes explicitly", async () => {
    const search = vi.fn(async () => response([region, location]));
    render(<Harness search={search} />);
    const input = screen.getByRole("combobox", { name: "지시 입력" });

    await userEvent.type(input, "@");
    fireEvent.click(await screen.findByRole("option", { name: /@영역 A/ }));
    await userEvent.type(input, "@");
    fireEvent.click(await screen.findByRole("option", { name: /@회복 지점/ }));
    const chips = screen.getByTestId("mention-chips");
    expect(within(chips).getAllByText(/^@/).map((node) => node.textContent)).toEqual([
      "@영역 A",
      "@회복 지점",
    ]);

    await userEvent.type(input, "@");
    fireEvent.click(await screen.findByRole("option", { name: /@영역 A/ }));
    expect(within(chips).getAllByText(/^@/)).toHaveLength(2);
    expect(screen.getByRole("alert")).toHaveTextContent("이미 선택");

    await userEvent.click(screen.getByRole("button", { name: "@영역 A 멘션 제거" }));
    expect(chips).not.toHaveTextContent("@영역 A");
  });

  it("invalidates unsent chips on project change and isolates scopes", async () => {
    const search = vi.fn(async () => response([region]));
    const view = render(<Harness search={search} project="project-a" scope="session-a" />);
    const input = screen.getByRole("combobox", { name: "지시 입력" });
    await userEvent.type(input, "@");
    fireEvent.click(await screen.findByRole("option", { name: /@영역 A/ }));

    view.rerender(<Harness search={search} project="project-b" scope="session-a" />);
    expect(await screen.findByText("만료됨")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("프로젝트가 변경");

    view.rerender(<Harness search={search} project="project-b" scope="session-b" />);
    await waitFor(() => expect(screen.queryByTestId("mention-chips")).toBeNull());
  });
});
