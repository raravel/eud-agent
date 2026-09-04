/**
 * Native-project availability notice.
 *
 * The App renders this banner only when the configured project cannot be read.
 * The text names the manifest/path recovery action without leaking raw errors.
 */
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { ConnectionNotice } from "@/components/ConnectionNotice";

describe("ConnectionNotice", () => {
  it("renders a native project status region", () => {
    render(<ConnectionNotice />);

    expect(
      screen.getByRole("status", { name: "Native 프로젝트 상태" }),
    ).toBeInTheDocument();
  });

  it("states that the native project cannot be opened", () => {
    render(<ConnectionNotice />);

    expect(screen.getByText(/Native 프로젝트를 열 수 없습니다/)).toBeInTheDocument();
    expect(screen.getByText(/project\.json/)).toBeInTheDocument();
  });
});
