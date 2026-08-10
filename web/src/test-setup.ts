/** Shared setup for component tests. */
import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';

// Vitest runs without global injection, so RTL's own auto-cleanup hook never
// registers. Without this, mounted trees accumulate across tests and queries
// start matching a previous test's DOM.
afterEach(cleanup);

// jsdom implements neither observer; layout has no meaning there (every rect
// is zero), so a no-op is honest.
class NoopObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
  takeRecords() {
    return [];
  }
}

globalThis.ResizeObserver ??= NoopObserver as unknown as typeof ResizeObserver;
globalThis.IntersectionObserver ??= NoopObserver as unknown as typeof IntersectionObserver;

// jsdom implements none of the pointer-capture API the drag controls use.
// Drag geometry is tested in the pure modules; components only need the
// calls to not throw.
Element.prototype.setPointerCapture ??= () => {};
Element.prototype.releasePointerCapture ??= () => {};
Element.prototype.hasPointerCapture ??= () => false;

// Older jsdom builds lack <dialog>'s imperative API; the Modal only needs
// open-state bookkeeping, not real top-layer rendering.
HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
  this.open = true;
};
HTMLDialogElement.prototype.close ??= function (this: HTMLDialogElement) {
  this.open = false;
};
