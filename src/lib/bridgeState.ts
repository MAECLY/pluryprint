/**
 * What Settings > Browser integration says about one browser (FR-127).
 */

import type { BrowserStatus } from '@/lib/ipc';

/** What one browser's row says. */
export type BrowserState = 'connected' | 'stale' | 'notConnected' | 'notFound';

/**
 * - `connected`: the manifest names this copy's bridge, which exists;
 * - `stale`: a manifest is there but names another copy (the app moved) or
 *   a bridge that is gone, so Reconnect fixes it;
 * - `notConnected`: the browser is here but nothing is registered;
 * - `notFound`: the browser is not installed for this user.
 */
export function browserState(s: BrowserStatus): BrowserState {
  if (s.registered && s.points_here && s.binary_exists) return 'connected';
  if (s.registered) return 'stale';
  return s.detected ? 'notConnected' : 'notFound';
}
