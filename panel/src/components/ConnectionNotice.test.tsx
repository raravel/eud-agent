/**
 * Editor connection-state notice (EUD-120).
 *
 * The App renders this presentational banner only when the backend reports the
 * EUD Editor bridge heartbeat as stale/absent. Korean UI text must explain that
 * the editor is disconnected without exposing raw backend marker strings.
 *
 * Contract (Step B implements `@/components/ConnectionNotice`):
 *   export interface ConnectionNoticeProps {}
 *   export function ConnectionNotice(props): JSX.Element;
 */
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { ConnectionNotice } from "@/components/ConnectionNotice";

describe("ConnectionNotice", () => {
  it("renders an editor connection status region", () => {
    render(<ConnectionNotice />);

    expect(
      screen.getByRole("status", { name: "에디터 연결 상태" }),
    ).toBeInTheDocument();
  });

  it("states that the editor is not connected", () => {
    render(<ConnectionNotice />);

    expect(screen.getByText(/에디터가 연결되지 않았습니다/)).toBeInTheDocument();
  });
});
