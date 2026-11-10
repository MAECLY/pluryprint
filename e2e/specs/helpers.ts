/** Shared helpers for the E2E specs (not a spec file — no `*.spec.ts` glob). */
import { $, browser } from '@wdio/globals';

/**
 * A fresh app launch shows the onboarding overlay (zIndex above the shell).
 * Dismiss it via its "Skip" button so the primary UI is interactive. A no-op
 * if onboarding was already completed.
 */
export async function dismissOnboarding(): Promise<void> {
  // Either language: on the first launch the UI follows the OS language
  // (FR-093), so on a Spanish Windows the button reads "Omitir" until
  // `forceEnglishUi` has run.
  let skip = await $('button=Skip');
  if (!(await skip.isExisting())) skip = await $('button=Omitir');
  if (await skip.isExisting()) {
    await skip.click();
    // Wait for the overlay to tear down before interacting with the shell.
    await skip
      .waitForExist({ reverse: true, timeout: 5_000 })
      .catch(() => undefined);
  }
}

/**
 * Click a left-nav item by its visible label (e.g. "Printers", "About").
 * Give the Spanish label too when the UI may be in Spanish at that point
 * (the sidebar is translated, FR-094).
 */
export async function navigateTo(
  label: string,
  spanish?: string,
): Promise<void> {
  let item = await $(`button*=${label}`);
  if (spanish && !(await item.isExisting())) {
    item = await $(`button*=${spanish}`);
  }
  await item.waitForClickable({ timeout: 10_000 });
  await item.click();
}

/**
 * The element whose own text contains `text` — any tag.
 *
 * WebdriverIO's bare `*=text` is a *partial link text* selector: it only
 * matches `<a>` elements, so `$('*=report.pdf')` never finds a row, a toast or
 * a list item. This matches on the DOM text (not the rendered, CSS-uppercased
 * one), which is what a spec means.
 */
export function byText(text: string) {
  if (text.includes('"')) throw new Error(`byText: no double quotes: ${text}`);
  return $(`//*[text()[contains(., "${text}")]]`);
}

/**
 * The page's visible text, lower-cased. `getText()` returns the *rendered*
 * text, so a heading styled `text-transform: uppercase` reads "KEYBOARD
 * SHORTCUTS"; compare against lower-cased expectations with this.
 */
export async function visibleText(): Promise<string> {
  return (await (await $('body')).getText()).toLowerCase();
}

/**
 * Replace a text input's value the way a user does: select all, then type.
 * `setValue` clears with a DOM write React never sees, so a controlled input
 * re-renders its old value and the typing is appended to it.
 */
export async function replaceInputText(
  input: WebdriverIO.Element,
  text: string,
): Promise<void> {
  await input.click();
  const modifier = process.platform === 'darwin' ? 'Meta' : 'Control';
  await browser.keys([modifier, 'a']);
  await browser.keys(['Backspace']);
  await input.addValue(text);
}

/** Call a Tauri command from the page, through the host's own IPC bridge. */
export async function invokeIpc(
  cmd: string,
  args: Record<string, unknown> = {},
): Promise<unknown> {
  return browser.execute(
    async (c: string, a: Record<string, unknown>) => {
      const internals = (
        window as unknown as {
          __TAURI_INTERNALS__: {
            invoke: (
              cmd: string,
              args: Record<string, unknown>,
            ) => Promise<unknown>;
          };
        }
      ).__TAURI_INTERNALS__;
      return internals.invoke(c, a);
    },
    cmd,
    args,
  );
}

/**
 * Pick a UI language on the Settings screen by its locale code.
 *
 * Driven by the option *values*, not by the visible labels, so it works
 * whichever language the app is currently showing — which is what a cleanup
 * hook needs after a test that switched to Spanish and then failed.
 */
export async function selectLanguage(locale: string): Promise<void> {
  await browser.execute((wanted: string) => {
    const selects = Array.from(document.querySelectorAll('select'));
    const language = selects.find((s) =>
      Array.from(s.options).some((o) => o.value === wanted),
    );
    if (!language) throw new Error('language select not found');
    language.value = wanted;
    language.dispatchEvent(new Event('change', { bubbles: true }));
  }, locale);
}

/**
 * Put the settings this suite touches back to the shipped defaults.
 *
 * These specs drive the *real built binary*, so they write the real per-user
 * `config.toml` of whoever runs them — there is no config-path override to
 * point at a temp directory (see `e2e/README.md`). Restoring is therefore not
 * politeness but isolation: the next spec, and the next run on the same
 * machine, start from a known state.
 *
 * Call it from an `afterEach`, never from the body of a test: a test that fails
 * halfway would otherwise leave the config dirty for everything after it. It
 * follows that this function must survive being called from any screen and in
 * any language, and must never throw — a cleanup failure would mask the real
 * failure the hook is running after.
 */
export async function restoreDefaultSettings(): Promise<void> {
  try {
    await dismissOnboarding();
    await navigateTo('Settings', 'Ajustes');
    // Language first: the appearance controls below are labelled in whatever
    // language is active, and "System"/"Comfortable" are the English ones.
    await selectLanguage('en-US');
    for (const label of ['System', 'Comfortable']) {
      const control = await $(`button*=${label}`);
      if (await control.isExisting()) await control.click();
    }
  } catch (error) {
    // Reported, not thrown: the test's own verdict must survive the cleanup.
    console.warn('could not restore the default settings:', error);
  }
}

/**
 * Switch the UI to English, whatever language it starts in.
 *
 * Every spec finds controls by their English labels, but the first launch
 * picks the OS language (FR-093) and a developer's machine may already hold a
 * Spanish `config.toml`. Without this the whole suite fails on a non-English
 * Windows, each lookup burning its full timeout. Runs once per session from
 * the wdio `before` hook. The Spanish labels are `onboarding.skip` and
 * `nav.settings` in `src/i18n/locales/es/home.json`.
 */
export async function forceEnglishUi(): Promise<void> {
  // The sidebar may still be rendering when the session starts. Wait for it
  // in either language before picking one: an early English miss would fall
  // back to "Ajustes" on an English UI and burn its whole timeout.
  const english = await $('button*=Settings');
  const spanish = await $('button*=Ajustes');
  await browser.waitUntil(
    async () => (await english.isExisting()) || (await spanish.isExisting()),
    { timeout: 15_000, timeoutMsg: 'the sidebar never rendered' },
  );
  // The tour waits for the config (App.tsx), so on a slow host (a debug
  // build under Xvfb on Linux) it can appear after the sidebar and cover it
  // just as Settings is clicked. Give it a moment to show up before
  // deciding there is none; this runs once per session, not per test.
  await browser
    .waitUntil(
      async () =>
        (await $('button=Skip').isExisting()) ||
        (await $('button=Omitir').isExisting()),
      { timeout: 5_000 },
    )
    .catch(() => undefined);
  await dismissOnboarding();
  const settings = (await english.isExisting()) ? english : spanish;
  await settings.waitForClickable({ timeout: 10_000 });
  await settings.click();
  await selectLanguage('en-US');
  await $('button*=Settings').waitForExist({ timeout: 10_000 });
}
