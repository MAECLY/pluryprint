import {
  render,
  screen,
  fireEvent,
  waitFor,
  within,
} from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';

import type { BridgeOverview, BrowserStatus } from '@/ipc/bindings';
import { I18nProvider } from '@/i18n/I18nProvider';
import { Toaster } from '@/components/AppShell/Toaster';
import { BrowserIntegrationSection } from './BrowserIntegrationSection';
import { browserState } from '@/lib/bridgeState';

const MANIFEST =
  '/Users/ana/Library/Application Support/Google/Chrome/NativeMessagingHosts/com.maecly.pluryprint.json';

function status(over: Partial<BrowserStatus>): BrowserStatus {
  return {
    browser: 'chrome',
    detected: true,
    manifest_path: MANIFEST,
    registered: true,
    points_here: true,
    binary_exists: true,
    allowed_origins: ['chrome-extension://ibgodfpcihlddbbfaoooehfgbmahipbj/'],
    ...over,
  };
}

function overview(over: Partial<BridgeOverview> = {}): BridgeOverview {
  return {
    platform: 'macos',
    installed: true,
    translocated: false,
    browsers: [
      status({ browser: 'chrome' }),
      status({ browser: 'edge', registered: true, points_here: false }),
      status({
        browser: 'brave',
        registered: false,
        points_here: false,
        binary_exists: false,
      }),
      status({
        browser: 'chromium',
        detected: false,
        registered: false,
        points_here: false,
        binary_exists: false,
      }),
    ],
    ...over,
  };
}

function mock(first: BridgeOverview, afterReconnect?: BridgeOverview) {
  const calls: string[] = [];
  mockIPC((cmd) => {
    calls.push(cmd);
    if (cmd === 'bridge_status') return first;
    if (cmd === 'bridge_reconnect') return afterReconnect ?? first;
    return null;
  });
  return calls;
}

function renderSection() {
  render(
    <I18nProvider>
      <BrowserIntegrationSection />
      <Toaster theme="light" />
    </I18nProvider>,
  );
}

function row(browser: string): HTMLElement {
  const item = document.querySelector<HTMLElement>(
    `li[data-browser="${browser}"]`,
  );
  if (!item) throw new Error(`no row for ${browser}`);
  return item;
}

afterEach(() => clearMocks());

describe('BrowserIntegrationSection', () => {
  it('FR-127: each browser row says whether Pluryfact can reach Pluryprint', async () => {
    mock(overview());
    renderSection();

    expect(await screen.findByText('Browser integration')).toBeInTheDocument();
    await waitFor(() => expect(row('chrome')).toBeInTheDocument());
    expect(
      within(row('chrome')).getByText('Google Chrome'),
    ).toBeInTheDocument();
    expect(within(row('chrome')).getByText('Connected')).toBeInTheDocument();
    expect(within(row('chrome')).getByText(MANIFEST)).toBeInTheDocument();
    expect(
      within(row('edge')).getByText('Needs reconnecting'),
    ).toBeInTheDocument();
    expect(within(row('brave')).getByText('Not connected')).toBeInTheDocument();
    expect(
      within(row('chromium')).getByText('Not installed'),
    ).toBeInTheDocument();
    // The platform's own note.
    expect(
      screen.getByText(/registers every browser it finds/),
    ).toBeInTheDocument();
  });

  it('FR-127: Reconnect registers again and shows the new state', async () => {
    const calls = mock(
      overview(),
      overview({
        browsers: overview().browsers.map((b) =>
          b.detected
            ? { ...b, registered: true, points_here: true, binary_exists: true }
            : b,
        ),
      }),
    );
    renderSection();
    const button = await screen.findByRole('button', { name: 'Reconnect' });
    fireEvent.click(button);

    await waitFor(() => expect(calls).toContain('bridge_reconnect'));
    expect(
      await screen.findByText('Browsers reconnected to Pluryprint.'),
    ).toBeInTheDocument();
    await waitFor(() =>
      expect(within(row('edge')).getByText('Connected')).toBeInTheDocument(),
    );
    expect(within(row('brave')).getByText('Connected')).toBeInTheDocument();
  });

  it('FR-127: a translocated copy explains itself and cannot register', async () => {
    mock(overview({ translocated: true }));
    renderSection();
    expect(await screen.findByRole('alert')).toHaveTextContent(
      /temporary location/,
    );
    expect(screen.getByRole('button', { name: 'Reconnect' })).toBeDisabled();
  });

  it('FR-127: a build without the bridge says so and cannot register', async () => {
    mock(overview({ installed: false, platform: 'windows' }));
    renderSection();
    expect(await screen.findByRole('alert')).toHaveTextContent(
      /does not include the browser bridge/,
    );
    expect(screen.getByRole('button', { name: 'Reconnect' })).toBeDisabled();
    expect(
      screen.getByText(/installer registers Chrome, Edge, Brave and Chromium/),
    ).toBeInTheDocument();
  });

  it('FR-127: outside the desktop app the section says it is unavailable', async () => {
    const w = window as unknown as {
      __TAURI_INTERNALS__?: unknown;
      __TAURI__?: unknown;
    };
    delete w.__TAURI_INTERNALS__;
    delete w.__TAURI__;
    renderSection();
    expect(
      await screen.findByText(/only available in the desktop app/),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Reconnect' })).toBeNull();
  });

  it('FR-127: the row state follows the registration', () => {
    expect(browserState(status({}))).toBe('connected');
    expect(browserState(status({ points_here: false }))).toBe('stale');
    expect(browserState(status({ binary_exists: false }))).toBe('stale');
    expect(browserState(status({ registered: false }))).toBe('notConnected');
    expect(browserState(status({ registered: false, detected: false }))).toBe(
      'notFound',
    );
  });
});
