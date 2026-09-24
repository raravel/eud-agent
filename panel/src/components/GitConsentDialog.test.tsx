/**
 * Repository consent, asked once when the project was ALREADY a git repository.
 *
 * Contract (`@/components/GitConsentDialog`):
 *   export interface GitConsentDialogProps {
 *     open: boolean;                              // consent === "pending"
 *     onDecide(granted: boolean): Promise<void>;  // git_consent_set
 *   }
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { GitConsentDialog } from "@/components/GitConsentDialog";

describe("GitConsentDialog", () => {
  it("stays closed while consent is not pending", () => {
    render(<GitConsentDialog open={false} onDecide={vi.fn()} />);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("explains that committing is how 되돌리기 works, and what declining means", async () => {
    render(<GitConsentDialog open onDecide={vi.fn()} />);
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("턴이 끝날 때마다 변경을 커밋");
    expect(dialog).toHaveTextContent("되돌리기");
    expect(dialog).toHaveTextContent("전적으로 사용자가 관리합니다");
  });

  it("grants consent", async () => {
    const onDecide = vi.fn(async () => {});
    render(<GitConsentDialog open onDecide={onDecide} />);

    await userEvent.click(
      await screen.findByRole("button", { name: "커밋을 허용합니다" }),
    );
    await waitFor(() => expect(onDecide).toHaveBeenCalledWith(true));
  });

  it("declines consent", async () => {
    const onDecide = vi.fn(async () => {});
    render(<GitConsentDialog open onDecide={onDecide} />);

    await userEvent.click(
      await screen.findByRole("button", { name: "직접 관리할게요" }),
    );
    await waitFor(() => expect(onDecide).toHaveBeenCalledWith(false));
  });

  it("keeps the question on screen when recording the answer fails", async () => {
    const onDecide = vi.fn(async () => {
      throw new Error("설정을 저장하지 못했습니다.");
    });
    render(<GitConsentDialog open onDecide={onDecide} />);

    await userEvent.click(
      await screen.findByRole("button", { name: "커밋을 허용합니다" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "설정을 저장하지 못했습니다.",
    );
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });
});
