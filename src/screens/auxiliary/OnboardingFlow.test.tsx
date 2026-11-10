/**
 * First-run tour tests. Hermetic: the profiles store is set directly, no IPC
 * host and no user config are involved.
 *
 * Half of these assert what the tour must NOT say. The shell integration is not
 * in this build, so a step promising it in the present tense — or a Settings
 * switch to turn it off — is a defect the moment it is written, and only a test
 * keeps it from coming back.
 */

import { render, screen, fireEvent } from '@testing-library/react';
import { describe, it, expect, beforeEach, vi } from 'vitest';

import { OnboardingFlow } from './OnboardingFlow';
import { shortcutLabel } from '@/components/AppShell/MPAppShell';
import { BUILT_IN_PROFILES } from '@/lib/profiles';
import { rememberHostInfo } from '@/lib/platform';
import { useProfilesStore } from '@/stores/profilesStore';

/** Walk the tour to the last step, returning the text seen along the way. */
function walkToEnd(): string {
  let seen = document.body.textContent ?? '';
  for (let i = 0; i < 10; i += 1) {
    const next = screen.queryByRole('button', { name: /continue/i });
    if (!next) break;
    fireEvent.click(next);
    seen += document.body.textContent ?? '';
  }
  return seen;
}

describe('OnboardingFlow', () => {
  beforeEach(() => {
    useProfilesStore.setState({
      status: 'ready',
      profiles: BUILT_IN_PROFILES,
      error: undefined,
    });
  });

  it('walks forward and back through every step', () => {
    render(<OnboardingFlow onComplete={vi.fn()} />);
    expect(screen.getByText(/welcome to pluryprint/i)).toBeInTheDocument();
    // The first step has nothing to go back to.
    expect(screen.queryByRole('button', { name: /back/i })).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /continue/i }));
    expect(screen.getByText(/detect your printers/i)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /back/i }));
    expect(screen.getByText(/welcome to pluryprint/i)).toBeInTheDocument();
  });

  it('completes from the last step and from Skip', () => {
    const onComplete = vi.fn();
    const { unmount } = render(<OnboardingFlow onComplete={onComplete} />);
    walkToEnd();
    fireEvent.click(screen.getByRole('button', { name: /get started/i }));
    expect(onComplete).toHaveBeenCalledTimes(1);
    unmount();

    render(<OnboardingFlow onComplete={onComplete} />);
    fireEvent.click(screen.getByRole('button', { name: /skip/i }));
    expect(onComplete).toHaveBeenCalledTimes(2);
  });

  it('FR-111/NFR-044: is a labelled modal dialog that takes the focus', () => {
    render(<OnboardingFlow onComplete={vi.fn()} />);
    const dialog = screen.getByRole('dialog', {
      name: /welcome to pluryprint/i,
    });
    expect(dialog).toHaveAttribute('aria-modal', 'true');
    expect(screen.getByRole('button', { name: /continue/i })).toHaveFocus();

    fireEvent.click(screen.getByRole('button', { name: /continue/i }));
    expect(screen.getByRole('button', { name: /continue/i })).toHaveFocus();
  });

  it('NFR-041: Escape skips the tour, and Tab stays inside it', () => {
    const onComplete = vi.fn();
    render(<OnboardingFlow onComplete={onComplete} />);
    const skip = screen.getByRole('button', { name: /skip/i });
    const next = screen.getByRole('button', { name: /continue/i });

    // Tab from the last control wraps to the first, never into the app.
    fireEvent.keyDown(document, { key: 'Tab' });
    expect(skip).toHaveFocus();
    fireEvent.keyDown(document, { key: 'Tab', shiftKey: true });
    expect(next).toHaveFocus();

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onComplete).toHaveBeenCalledTimes(1);
  });

  it('NFR-041: announces which step is showing', () => {
    render(<OnboardingFlow onComplete={vi.fn()} />);
    expect(screen.getByRole('status')).toHaveAccessibleName('Step 1 of 5');
    fireEvent.click(screen.getByRole('button', { name: /continue/i }));
    expect(screen.getByRole('status')).toHaveAccessibleName('Step 2 of 5');
  });

  it('does not promise the shell right-click menu, nor a Settings switch for it', () => {
    render(<OnboardingFlow onComplete={vi.fn()} />);
    const seen = walkToEnd();
    expect(seen).not.toMatch(/adds a "print with pluryprint" entry/i);
    expect(seen).not.toMatch(/disable it any time in settings/i);
    expect(seen).not.toMatch(/right-click any number of files/i);
    // Off macOS the installer's Explorer entry is named, where Windows 11
    // really puts it.
    expect(seen).not.toMatch(/arrives once an installer registers it/i);
    expect(seen).toMatch(
      /In Explorer, right-click files and choose Pluryprint/,
    );
    expect(seen).toMatch(/on Windows 11, under Show more options/);
    // And "Save as PDF" is not promised as a printer on the list.
    expect(seen).not.toMatch(/'Save as PDF' on Mac/);
  });

  it('FR-001: on macOS the tour points at Finder › Services, which ships', () => {
    const platform = vi
      .spyOn(window.navigator, 'platform', 'get')
      .mockReturnValue('MacIntel');
    try {
      render(<OnboardingFlow onComplete={vi.fn()} />);
      const seen = walkToEnd();
      expect(seen).toMatch(/Services › Print with Pluryprint/);
      expect(seen).not.toMatch(/Show more options/);
      // The macOS app binary is itself the CLI.
      expect(seen).toMatch(/pluryprint print --files/);
      expect(seen).toMatch(/Save as PDF in your apps' own print dialog/);
    } finally {
      platform.mockRestore();
    }
  });

  it('FR-001: on Linux the tour names CUPS, Print to File and the file manager', () => {
    rememberHostInfo({
      os: 'linux',
      arch: 'x86_64',
      executable: '/usr/bin/pluryprint',
    });
    render(<OnboardingFlow onComplete={vi.fn()} />);
    const seen = walkToEnd();
    expect(seen).toMatch(/printer set up in CUPS/);
    expect(seen).toMatch(/Print to File in your apps' own print dialog/);
    expect(seen).toMatch(/choose Pluryprint under Open With/);
    // The .deb puts the app's own executable on PATH.
    expect(seen).toMatch(/pluryprint print --files/);
    expect(seen).not.toMatch(/Explorer|Finder|Microsoft Print to PDF/);
    expect(seen).not.toMatch(/pluryprint-cli/);
  });

  it('FR-001: before the backend answers, a Linux webview already gets Linux copy', () => {
    const platform = vi
      .spyOn(window.navigator, 'platform', 'get')
      .mockReturnValue('Linux aarch64');
    try {
      render(<OnboardingFlow onComplete={vi.fn()} />);
      const seen = walkToEnd();
      // The package is not known yet, so both are described.
      expect(seen).toMatch(/Installed from the \.deb package/);
      expect(seen).not.toMatch(/Explorer|Finder/);
    } finally {
      platform.mockRestore();
    }
  });

  it('FR-008/FR-009: advertises the CLI, by its console name off macOS', () => {
    render(<OnboardingFlow onComplete={vi.fn()} />);
    const seen = walkToEnd();
    expect(seen).toMatch(/pluryprint-cli print --files/);
  });

  it('NFR-034: points at a shortcut that exists', () => {
    render(<OnboardingFlow onComplete={vi.fn()} />);
    const seen = walkToEnd();
    expect(seen).toContain(shortcutLabel('newJob'));
  });

  it('names the real default profile instead of a hardcoded one', () => {
    useProfilesStore.setState({
      status: 'ready',
      profiles: BUILT_IN_PROFILES.map((p) => ({
        ...p,
        default: p.id === 'builtin-photo-color',
      })),
      error: undefined,
    });
    render(<OnboardingFlow onComplete={vi.fn()} />);
    const seen = walkToEnd();
    expect(seen).toMatch(/Photo Color is the default profile/);
    expect(seen).not.toMatch(/Quick \(color, simplex, A4\)/);
  });
});
