/**
 * Vitest setup for component-level tests (@testing-library/react + happy-dom).
 *
 * - Registers jest-dom matchers (toBeDisabled / toHaveTextContent / ...).
 * - Auto-cleans the React tree after each test.
 * - Polyfills the DOM APIs Radix UI primitives call but happy-dom lacks
 *   (pointer capture + scrollIntoView). Without these, opening a Radix
 *   <Select> throws in happy-dom; the panel picker is built on it.
 *
 * Loaded via vitest.config.ts `test.setupFiles`. Plain side-effect module.
 */
import "@testing-library/jest-dom/vitest";
import { afterEach, vi } from "vitest";
import { cleanup } from "@testing-library/react";

vi.mock("@streamdown/mermaid", () => ({
  mermaid: {
    name: "mermaid",
    type: "diagram",
    language: "mermaid",
    getMermaid: () => ({
      initialize: () => {},
      render: async () => ({
        svg: '<svg xmlns="http://www.w3.org/2000/svg"><text>Rendered Mermaid</text></svg>',
      }),
    }),
  },
}));

afterEach(() => {
  cleanup();
});

// Radix primitives rely on Pointer Capture; happy-dom does not implement it.
if (!Element.prototype.hasPointerCapture) {
  Element.prototype.hasPointerCapture = () => false;
}
if (!Element.prototype.setPointerCapture) {
  Element.prototype.setPointerCapture = () => {};
}
if (!Element.prototype.releasePointerCapture) {
  Element.prototype.releasePointerCapture = () => {};
}
// Radix Select scrolls the active item into view on open.
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => {};
}

// Streamdown defers Mermaid work until the diagram approaches the viewport.
// Component tests have no layout engine, so report observed nodes as visible.
globalThis.IntersectionObserver = class ImmediateIntersectionObserver {
  readonly root = null;
  readonly rootMargin = "0px";
  readonly thresholds = [0];

  constructor(private readonly callback: IntersectionObserverCallback) {}

  observe(target: Element): void {
    this.callback(
      [
        {
          isIntersecting: true,
          target,
        } as IntersectionObserverEntry,
      ],
      this as unknown as IntersectionObserver,
    );
  }

  disconnect(): void {}
  unobserve(): void {}
  takeRecords(): IntersectionObserverEntry[] {
    return [];
  }
} as unknown as typeof IntersectionObserver;
