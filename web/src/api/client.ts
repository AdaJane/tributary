/**
 * Daemon endpoints and the typed REST client. All request/response types
 * come from the generated schema — no hand-written DTOs. No auth: the
 * daemon binds loopback and trusts its own machine (origin checks fence
 * hostile web pages).
 */
import createClient from 'openapi-fetch';

import type { paths } from './generated/schema';

const DEFAULT_BASE = 'http://127.0.0.1:4600';

// Production builds are served by the daemon itself (embed-ui), so the
// API is same-origin; dev servers and vitest keep the canonical pair.
export const API_BASE_URL: string =
  import.meta.env.VITE_TRIBD_URL ??
  (import.meta.env.DEV ? DEFAULT_BASE : window.location.origin);

export const WS_URL = `${API_BASE_URL.replace(/^http/, 'ws')}/ws`;

/** The one-way binary monitor stream (see `monitor_pump` in the daemon). */
export const MONITOR_WS_URL = `${WS_URL}/monitor`;

export const $api = createClient<paths>({ baseUrl: API_BASE_URL });
