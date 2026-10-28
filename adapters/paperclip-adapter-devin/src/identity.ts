/**
 * The adapter's type key, in its own module.
 *
 * Kept separate from `index.ts` so server-side modules can import it without
 * pulling in the generated model catalogue that the config form needs.
 */
export const ADAPTER_TYPE = "devin_local";
