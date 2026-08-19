import { renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { useProjectIdentityEffect } from "./projectIdentity";

describe("useProjectIdentityEffect", () => {
  it("does not reset project state when only the callback identity changes", () => {
    const firstEffect = vi.fn();
    const replacementEffect = vi.fn();
    const { rerender } = renderHook(
      ({ project, effect }) => useProjectIdentityEffect(project, effect),
      {
        initialProps: {
          project: "Map A" as string | null,
          effect: firstEffect,
        },
      },
    );

    expect(firstEffect).toHaveBeenCalledOnce();
    expect(firstEffect).toHaveBeenCalledWith("Map A");

    rerender({ project: "Map A", effect: replacementEffect });
    expect(replacementEffect).not.toHaveBeenCalled();

    rerender({ project: "Map B", effect: replacementEffect });
    expect(replacementEffect).toHaveBeenCalledOnce();
    expect(replacementEffect).toHaveBeenCalledWith("Map B");
  });
});
