import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import i18n, { setPanelLanguage, type UiLanguage, type UiLanguageState } from "@/lib/i18n";

import { LanguageSettingsPanel } from "./LanguageSettingsPanel";

afterEach(() => {
  act(() => setPanelLanguage("ko"));
});

describe("LanguageSettingsPanel", () => {
  it("shows the OS-following state and saves an explicit choice", async () => {
    const load = vi.fn(async (): Promise<UiLanguageState> => ({ language: "ko", chosen: false }));
    const save = vi.fn(async (language: UiLanguage | null): Promise<UiLanguageState> => {
      setPanelLanguage(language ?? "ko");
      return { language: language ?? "ko", chosen: language !== null };
    });
    render(<LanguageSettingsPanel load={load} save={save} />);

    const system = await screen.findByRole("radio", { name: /시스템 언어 따르기/ });
    await waitFor(() => expect(system).toBeChecked());
    expect(screen.getByText("지금은 한국어")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("radio", { name: "English" }));

    await waitFor(() => expect(save).toHaveBeenCalledWith("en"));
    expect(i18n.language).toBe("en");
    expect(await screen.findByRole("heading", { name: "Language" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "English" })).toBeChecked();
  });

  it("reports a failed save with a recovery action", async () => {
    render(
      <LanguageSettingsPanel
        load={async () => ({ language: "ko", chosen: true })}
        save={async () => {
          throw new Error("disk full");
        }}
      />,
    );
    await waitFor(() => expect(screen.getByRole("radio", { name: "한국어" })).toBeChecked());
    fireEvent.click(screen.getByRole("radio", { name: "English" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("다시 시도하세요");
  });
});
