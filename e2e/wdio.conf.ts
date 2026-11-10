/**
 * WebdriverIO configuration for Pluryprint end-to-end tests.
 *
 * These drive the *real built binary* through `tauri-driver` (the WebDriver
 * bridge Tauri ships), not the Vite dev server. `tauri-driver` is a cargo
 * binary (`cargo install tauri-driver --locked`). On Windows it proxies to the
 * Microsoft Edge WebDriver (`msedgedriver.exe`) that matches the installed
 * WebView2 runtime — pass its path via the NATIVE_DRIVER env var. On Linux it
 * proxies to WebKitGTK's `WebKitWebDriver` (package `webkit2gtk-driver`),
 * which it finds on PATH.
 *
 * tauri-driver has no macOS support (WKWebView exposes no WebDriver), so this
 * suite runs on Windows and Linux only. CI runs it on Linux ARM64 after every
 * merge (linux.yml, non-blocking, under xvfb-run); run it locally on Windows
 * with `pnpm test:e2e` after a `pnpm tauri build --debug`.
 */

import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdir } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { browser } from '@wdio/globals';

const here = path.dirname(fileURLToPath(import.meta.url));
const artifactsDir = path.resolve(here, 'artifacts');
const isWindows = process.platform === 'win32';
const exe = isWindows ? '.exe' : '';

// The binary under test. CI builds with `pnpm tauri build --debug`, which lands
// in target/debug; override with TAURI_APP_PATH to point at a release build.
const application =
  process.env.TAURI_APP_PATH ??
  path.resolve(here, '..', 'src-tauri', 'target', 'debug', `pluryprint${exe}`);

/** Prefer `tauri-driver` on PATH, else the default cargo bin location. */
function tauriDriverBin(): string {
  const cargoBin = path.resolve(
    os.homedir(),
    '.cargo',
    'bin',
    `tauri-driver${exe}`,
  );
  return existsSync(cargoBin) ? cargoBin : `tauri-driver${exe}`;
}

let tauriDriver: ChildProcess | undefined;

export const config: WebdriverIO.Config = {
  runner: 'local',

  // tauri-driver listens on 4444 and speaks plain WebDriver.
  hostname: '127.0.0.1',
  port: 4444,
  path: '/',

  specs: [path.resolve(here, 'specs', '**', '*.e2e.ts')],
  maxInstances: 1,

  capabilities: [
    {
      // Consumed by tauri-driver to launch the app under test.
      'tauri:options': { application },
      // tauri-driver ignores browserName, but wdio requires a value.
      browserName: 'wry',
      // WebDriver BiDi does not reach the app's WebView2 through
      // msedgedriver: its commands target a browsing context that does not
      // exist ("Cannot find context with specified id") and the specs read a
      // blank page. Classic WebDriver addresses the window that is there.
      'wdio:enforceWebDriverClassic': true,
    } as WebdriverIO.Capabilities,
  ],

  logLevel: 'warn',
  bail: 0,
  waitforTimeout: 15_000,
  connectionRetryTimeout: 120_000,
  connectionRetryCount: 3,

  framework: 'mocha',
  reporters: ['spec'],
  mochaOpts: { ui: 'bdd', timeout: 60_000 },

  // Start tauri-driver once for the whole run; each spec gets a fresh app
  // session (and thus a fresh onboarding overlay) from it.
  onPrepare: () => {
    const driverArgs: string[] = [];
    const nativeDriver = process.env.NATIVE_DRIVER;
    if (nativeDriver) driverArgs.push('--native-driver', nativeDriver);
    tauriDriver = spawn(tauriDriverBin(), driverArgs, {
      stdio: [null, process.stdout, process.stderr],
    });
    tauriDriver.on('error', (err) => {
      // Without the driver there is nothing to test; fail loudly.
      console.error('Failed to start tauri-driver:', err);
      process.exit(1);
    });
  },

  // Every spec addresses controls by their English labels; switch the UI to
  // English first so a Spanish host (or config) does not fail the suite.
  before: async () => {
    const { forceEnglishUi } = await import('./specs/helpers.js');
    await forceEnglishUi();
  },

  // Capture a screenshot into e2e/artifacts/ for any failing test, so a CI
  // failure has a visual to triage from (uploaded as an artifact).
  afterTest: async (test, _context, { passed }) => {
    if (passed) return;
    await mkdir(artifactsDir, { recursive: true });
    const safe = test.title.replace(/[^\w.-]+/g, '_').slice(0, 80);
    await browser.saveScreenshot(
      path.join(artifactsDir, `${safe}-${Date.now()}.png`),
    );
  },

  // Linux: ending a session closes the window, but WebKitWebDriver leaves
  // the app process running (seen with tauri-driver 2.1.0 and WebKitGTK
  // 2.50). The next session's launch is then handed to that process by the
  // single-instance plugin and exits, and session creation times out four
  // times over (eight minutes per spec). End what this session started (and
  // any other Pluryprint on this machine: do not run the suite next to one).
  afterSession: () => {
    if (process.platform !== 'linux') return;
    spawnSync('pkill', ['-KILL', '-x', path.basename(application)], {
      stdio: 'ignore',
    });
  },

  onComplete: () => {
    tauriDriver?.kill();
  },
};
