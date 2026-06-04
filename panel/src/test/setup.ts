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
import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";

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
