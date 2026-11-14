import { useEffect, useState } from 'react';
import { toast } from 'sonner';

import { MPBadge, MPButton, MPSectionLabel } from '@/components/ui';
import type { BadgeTone } from '@/components/ui/Badge';
import { useT } from '@/i18n/I18nProvider';
import { browserState, type BrowserState } from '@/lib/bridgeState';
import {
  bridgeReconnect,
  bridgeStatus,
  isTauri,
  type BridgeOverview,
  type Browser,
} from '@/lib/ipc';

/** Browser names are product names: the same in every language. */
const BROWSER_NAMES: Record<Browser, string> = {
  chrome: 'Google Chrome',
  edge: 'Microsoft Edge',
  brave: 'Brave',
  chromium: 'Chromium',
};

const TONE: Record<BrowserState, BadgeTone> = {
  connected: 'success',
  stale: 'warn',
  notConnected: 'neutral',
  notFound: 'neutral',
};

const noteStyle = {
  fontSize: 11.5,
  color: 'var(--mp-textMuted)',
  lineHeight: 1.5,
  padding: '12px 0',
} as const;

/**
 * Settings > Browser integration (FR-127): whether PluryFact, the SRI
 * invoice downloader for Chrome, Edge and Brave, can reach Pluryprint
 * through the browser bridge, per browser, and a Reconnect that registers
 * the bridge again. Nothing here opens a port or talks to the network: the
 * browser starts the bridge itself, and only for PluryFact.
 */
export function BrowserIntegrationSection() {
  const t = useT('settings');
  const [overview, setOverview] = useState<BridgeOverview | null>(null);
  // Outside the desktop host there is nothing to ask.
  const [status, setStatus] = useState<'loading' | 'ready' | 'unavailable'>(
    () => (isTauri() ? 'loading' : 'unavailable'),
  );
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    bridgeStatus()
      .then((o) => {
        if (cancelled) return;
        if (o && Array.isArray(o.browsers)) {
          setOverview(o);
          setStatus('ready');
        } else {
          setStatus('unavailable');
        }
      })
      .catch(() => {
        if (!cancelled) setStatus('unavailable');
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const reconnect = async () => {
    setBusy(true);
    try {
      const o = await bridgeReconnect();
      setOverview(o);
      setStatus('ready');
      toast.success(t('browserIntegration.reconnected'));
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section aria-labelledby="settings-browser-integration">
      <div style={{ height: 24 }} />
      <MPSectionLabel id="settings-browser-integration">
        {t('sections.browserIntegration')}
      </MPSectionLabel>
      <div style={noteStyle}>{t('browserIntegration.intro')}</div>

      {status === 'loading' && (
        <div style={noteStyle}>{t('browserIntegration.loading')}</div>
      )}
      {status === 'unavailable' && (
        <div style={noteStyle}>{t('browserIntegration.unavailable')}</div>
      )}
      {status === 'ready' && overview && (
        <>
          {overview.translocated && (
            <div role="alert" style={{ ...noteStyle, color: 'var(--mp-warn)' }}>
              {t('browserIntegration.translocated')}
            </div>
          )}
          {!overview.installed && !overview.translocated && (
            <div role="alert" style={{ ...noteStyle, color: 'var(--mp-warn)' }}>
              {t('browserIntegration.notInstalled')}
            </div>
          )}
          <ul
            aria-label={t('sections.browserIntegration')}
            style={{ listStyle: 'none', margin: 0, padding: 0 }}
          >
            {overview.browsers.map((b) => {
              const state = browserState(b);
              return (
                <li
                  key={b.browser}
                  data-browser={b.browser}
                  style={{
                    display: 'grid',
                    gridTemplateColumns: '1fr auto',
                    gap: 12,
                    alignItems: 'center',
                    padding: '10px 0',
                    borderBottom: '1px solid var(--mp-border)',
                  }}
                >
                  <div style={{ minWidth: 0 }}>
                    <div style={{ fontSize: 13, fontWeight: 500 }}>
                      {BROWSER_NAMES[b.browser]}
                    </div>
                    {state !== 'notFound' && b.registered && (
                      <div
                        className="mp-mono"
                        title={b.manifest_path}
                        style={{
                          fontSize: 11,
                          color: 'var(--mp-textMuted)',
                          marginTop: 2,
                          overflow: 'hidden',
                          textOverflow: 'ellipsis',
                          whiteSpace: 'nowrap',
                        }}
                      >
                        {b.manifest_path}
                      </div>
                    )}
                  </div>
                  <MPBadge tone={TONE[state]}>
                    {t(`browserIntegration.state.${state}`)}
                  </MPBadge>
                </li>
              );
            })}
          </ul>
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 12,
              padding: '12px 0',
            }}
          >
            <MPButton
              variant="secondary"
              size="sm"
              icon="rotate"
              onClick={() => void reconnect()}
              disabled={busy || overview.translocated || !overview.installed}
            >
              {t('browserIntegration.reconnect')}
            </MPButton>
            <span style={{ fontSize: 11.5, color: 'var(--mp-textMuted)' }}>
              {t(
                overview.platform === 'windows'
                  ? 'browserIntegration.hint'
                  : overview.platform === 'macos'
                    ? 'browserIntegration.hintMac'
                    : 'browserIntegration.hintLinux',
              )}
            </span>
          </div>
        </>
      )}
    </section>
  );
}
