/**
 * Console capacities the daemon enforces, mirrored so the UI can block a
 * control *before* the request rather than print a refusal after it.
 *
 * The daemon stays authoritative: these are for disabling buttons and
 * writing the reason beside them, never for deciding what is legal.
 */

/** `trib_core::MAX_STRIPS`. */
export const MAX_STRIPS = 32;

/** `trib_core::MAX_INSTRUMENTS`. */
export const MAX_INSTRUMENTS = 8;
