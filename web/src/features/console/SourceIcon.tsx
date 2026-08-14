import type { ReactNode } from 'react';

import type { SourceKind } from './devices';

/** Drawn-not-photographed glyphs, one per source kind — same idiom as the
 * jack socket. Stroke inherits the surrounding print color. */
const GLYPHS: Record<SourceKind, ReactNode> = {
  // A loop arrow: the default box follows whatever the system picks.
  default: (
    <>
      <path d="M5 12a7 7 0 1 1 2 5" />
      <path d="M7 21v-4h4" />
    </>
  ),
  mic: (
    <>
      <rect x="9" y="2" width="6" height="11" rx="3" />
      <path d="M5 11a7 7 0 0 0 14 0" />
      <path d="M12 18v3M8 21h8" />
    </>
  ),
  webcam: (
    <>
      <circle cx="12" cy="10" r="6" />
      <circle cx="12" cy="10" r="2" />
      <path d="M8 21l2.5-5.5M16 21l-2.5-5.5M8 21h8" />
    </>
  ),
  // The USB trident.
  usb: (
    <>
      <path d="M12 22V5M12 5l-2.5 3M12 5l2.5 3" />
      <path d="M12 15l-5-2.5V9M12 12l5-2.5V7" />
      <rect x="5.5" y="7" width="3" height="3" />
      <circle cx="17" cy="5.5" r="1.5" />
    </>
  ),
  // A quarter-inch plug for anything unrecognized.
  line: (
    <>
      <path d="M10 13V4a2 2 0 0 1 4 0v9" />
      <rect x="8" y="13" width="8" height="8" rx="1" />
    </>
  ),
  // Three white keys and two black: the one source the box makes itself.
  // Hand-drawn like the socket, because the patchbay draws hardware.
  instrument: (
    <>
      <rect x="3" y="6" width="18" height="12" rx="1" />
      <path d="M9 6v12M15 6v12" />
      <path d="M7.5 6v6h3V6M13.5 6v6h3V6" />
    </>
  ),
};

export function SourceIcon({ kind }: { kind: SourceKind }) {
  return (
    <svg
      viewBox="0 0 24 24"
      width="18"
      height="18"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      {GLYPHS[kind]}
    </svg>
  );
}
