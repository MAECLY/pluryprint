import { useEffect, useId, useState, type ReactNode } from 'react';
import {
  MPInput,
  MPSectionLabel,
  MPSegmented,
  MPSelect,
  MPSwitch,
  MPTopBar,
} from '@/components/ui';
import type { Density, ResolvedTheme, ThemeMode } from '@/lib/theme';
import type { UpdateChannel } from '@/lib/ipc';
import type { Bounds } from '@/ipc/bindings';
import {
  SUPPORTED_LOCALES,
  matchLocale,
  type Locale,
  type TranslationKey,
} from '@/i18n';
import { ConfirmDialog, type ConfirmState } from '@/components/ConfirmDialog';
import { useLocale, useT, type TFunction } from '@/i18n/I18nProvider';
import { useSettingsStore } from '@/stores/settingsStore';
import { useProfilesStore } from '@/stores/profilesStore';
import { useHostPlatform, type HostPlatform } from '@/lib/platform';
import { BrowserIntegrationSection } from '@/screens/settings/BrowserIntegrationSection';

interface SettingsScreenProps {
  /** Resolved (light/dark) theme — always present for the 2-way fallback. */
  theme: ResolvedTheme;
  setTheme: (t: ResolvedTheme) => void;
  density: Density;
  setDensity: (d: Density) => void;
  /**
   * Optional 3-way theme mode. When `mode`/`setMode` are supplied the theme
   * control renders Light/Dark/System and drives `setMode`; otherwise it falls
   * back to the 2-way Light/Dark control on `setTheme`.
   */
  mode?: ThemeMode;
  setMode?: (m: ThemeMode) => void;
}

/**
 * FR-086: how far each channel is from final releases. Moving to a lower
 * rank (towards stable) asks first, because it does not take the installed
 * version back down: the app stays on what it has until the new channel
 * reaches it.
 */
const CHANNEL_RANK: Record<UpdateChannel, number> = {
  stable: 0,
  beta: 1,
  nightly: 2,
};

/** A channel's translated name, as a table so a new channel must be named. */
const CHANNEL_LABEL = {
  stable: 'updates.channel.stable',
  beta: 'updates.channel.beta',
  nightly: 'updates.channel.nightly',
} as const satisfies Record<UpdateChannel, TranslationKey<'settings'>>;

/** Language names are endonyms, so the list reads the same in every locale. */
const LOCALE_LABELS: Record<Locale, string> = {
  'en-US': 'English',
  'es-ES': 'Español',
};

interface RowProps {
  label: string;
  hint?: string;
  /**
   * Dim the control and make it really unavailable (backend not reachable,
   * settings still loading): a disabled <fieldset> natively disables every
   * button, input and select inside, so they leave the tab order, ignore
   * keys and are announced as unavailable (NFR-041) — not just unclickable.
   */
  disabled?: boolean;
  children: ReactNode;
}

function Row({ label, hint, disabled, children }: RowProps) {
  return (
    <div
      style={{
        display: 'grid',
        gridTemplateColumns: '1fr auto',
        // Density is published on the document root by `src/lib/theme.ts`; the
        // fallbacks keep the row sized when this screen renders outside the app
        // shell (isolated tests, the standalone preview).
        gap: 'var(--mp-row-gap, 20px)',
        padding: 'var(--mp-row-py, 14px) 0',
        borderBottom: '1px solid var(--mp-border)',
        alignItems: 'center',
      }}
    >
      <div>
        <div style={{ fontSize: 13, fontWeight: 500 }}>{label}</div>
        {hint && (
          <div
            style={{
              fontSize: 11.5,
              color: 'var(--mp-textMuted)',
              marginTop: 2,
            }}
          >
            {hint}
          </div>
        )}
      </div>
      {/* A disabled fieldset takes its controls out of the tab order and
          announces them as unavailable (NFR-041); `inert` also covers custom
          widgets that are not form controls, and no pointer gets through.
          `pointer-events` alone would only stop the mouse: `inert` takes the
          whole subtree out of focus, interaction and the accessibility tree
          (React 19 renders the boolean prop as the native attribute). */}
      <fieldset
        disabled={disabled}
        inert={disabled ?? false}
        aria-disabled={disabled ? true : undefined}
        style={{
          border: 'none',
          margin: 0,
          padding: 0,
          minWidth: 0,
          ...(disabled ? { opacity: 0.45, pointerEvents: 'none' } : {}),
        }}
      >
        {children}
      </fieldset>
    </div>
  );
}

/** A full-width muted note (milestone caveats, consent copy, load states). */
function Note({ children }: { children: ReactNode }) {
  return (
    <div
      style={{
        fontSize: 11.5,
        color: 'var(--mp-textMuted)',
        lineHeight: 1.5,
        padding: '12px 0',
      }}
    >
      {children}
    </div>
  );
}

/** An inline, link-styled retry action inside a {@link Note}. */
function RetryButton({
  label,
  onClick,
}: {
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      style={{
        background: 'none',
        border: 'none',
        padding: 0,
        color: 'var(--mp-accent)',
        cursor: 'pointer',
        font: 'inherit',
        textDecoration: 'underline',
      }}
    >
      {label}
    </button>
  );
}

/**
 * The banner shown when `config.toml` exists but could not be parsed (FR-101).
 * The backend degraded to defaults for this read and refuses every write until
 * the file is fixed, so the values below are NOT the user's settings — saying
 * so here, at load, is the difference between a recoverable error and a screen
 * that quietly lies and then bounces the first change.
 */
function ConfigWarning({
  path,
  t,
}: {
  path: string;
  t: TFunction<'settings'>;
}) {
  return (
    <div
      role="alert"
      style={{
        margin: '12px 0',
        padding: '10px 12px',
        borderRadius: 8,
        border: '1px solid var(--mp-warn)',
        background: 'var(--mp-warnSoft)',
        color: 'var(--mp-text)',
        fontSize: 12,
        lineHeight: 1.5,
      }}
    >
      <div style={{ fontWeight: 600, marginBottom: 2 }}>
        {t('config.corruptTitle')}
      </div>
      <div>{t('config.corruptBody')}</div>
      <div
        style={{
          marginTop: 6,
          fontFamily: 'var(--mp-font-mono)',
          fontSize: 11.5,
          wordBreak: 'break-all',
        }}
      >
        {path}
      </div>
    </div>
  );
}

interface EngineFieldProps {
  label: string;
  hint: string;
  suffix: string;
  /** `undefined` until the persisted settings arrive (the field is disabled). */
  value: number | undefined;
  /**
   * The server's bounds for this knob (`AppSettings.meta.limits`). `undefined`
   * — a backend that predates them — falls back to server-side validation only.
   */
  bounds: Bounds | undefined;
  errorHint: string;
  onCommit: (n: number) => void;
  disabled: boolean;
}

/** Mirrors the Rust `Bounds::accepts`: the only client-side copy of the rule. */
/**
 * FR-001: where the right-click entry lives on this platform. macOS ships a
 * Finder Service; Windows an Explorer entry; on Linux it depends on the
 * package — the .deb registers "Open with" and the Dolphin and Nemo actions,
 * an AppImage nothing until the user integrates it with their desktop.
 */
function shellNoteKey(
  host: HostPlatform,
):
  | 'printing.shellNote'
  | 'printing.shellNoteMac'
  | 'printing.shellNoteLinux'
  | 'printing.shellNoteLinuxAppImage'
  | 'printing.shellNoteLinuxUnknown' {
  if (host.os === 'macos') return 'printing.shellNoteMac';
  if (host.os !== 'linux') return 'printing.shellNote';
  if (host.linuxPackage === 'deb') return 'printing.shellNoteLinux';
  if (host.linuxPackage === 'appimage')
    return 'printing.shellNoteLinuxAppImage';
  return 'printing.shellNoteLinuxUnknown';
}

function accepts(bounds: Bounds | undefined, n: number): boolean {
  if (!bounds) return true;
  if (n === 0 && bounds.zero === 'disables') return true;
  return n >= bounds.min && n <= bounds.max;
}

/**
 * A numeric engine setting with LOCAL text state. Typing never persists; the
 * value is committed on blur or Enter, and only when it parses to an in-bounds
 * whole number — an out-of-range entry shows an inline error and is NOT sent to
 * the backend. The field re-syncs to `value` whenever the persisted setting
 * changes (initial load, or a server clamp echoed back).
 */
function EngineField({
  label,
  hint,
  suffix,
  value,
  bounds,
  errorHint,
  onCommit,
  disabled,
}: EngineFieldProps) {
  const [text, setText] = useState(value === undefined ? '' : String(value));
  const [error, setError] = useState(false);
  const errorId = useId();

  // Re-synced while rendering, so the stale text never paints.
  const [syncedValue, setSyncedValue] = useState(value);
  if (syncedValue !== value) {
    setSyncedValue(value);
    setText(value === undefined ? '' : String(value));
    setError(false);
  }

  const commit = () => {
    const s = text.trim();
    const n = Number(s);
    if (!/^\d+$/.test(s) || !accepts(bounds, n)) {
      setError(true);
      return;
    }
    setError(false);
    if (n !== value) onCommit(n);
  };

  return (
    <Row label={label} hint={hint} disabled={disabled}>
      {/* onBlur bubbles from the inner input (React focusout), so it fires
          when the field loses focus even though MPInput owns no blur prop. */}
      <div
        onBlur={commit}
        style={{
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'flex-end',
          gap: 4,
        }}
      >
        <MPInput
          mono
          aria-label={label}
          {...(error ? { 'aria-describedby': errorId } : {})}
          value={text}
          suffix={suffix}
          error={error}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') commit();
          }}
        />
        {error && (
          <div
            id={errorId}
            style={{ fontSize: 11, color: 'var(--mp-error)' }}
            role="alert"
          >
            {errorHint}
          </div>
        )}
      </div>
    </Row>
  );
}

export function SettingsScreen({
  theme,
  setTheme,
  density,
  setDensity,
  mode,
  setMode,
}: SettingsScreenProps) {
  const t = useT('settings');
  const host = useHostPlatform();
  const locale = useLocale();
  const settings = useSettingsStore((s) => s.settings);
  const status = useSettingsStore((s) => s.status);
  const initSettings = useSettingsStore((s) => s.init);
  const patch = useSettingsStore((s) => s.patch);

  const profiles = useProfilesStore((s) => s.profiles);
  const profilesStatus = useProfilesStore((s) => s.status);
  const setDefaultProfile = useProfilesStore((s) => s.setDefault);
  const initProfiles = useProfilesStore((s) => s.init);

  useEffect(() => {
    void initSettings();
  }, [initSettings]);
  useEffect(() => {
    void initProfiles();
  }, [initProfiles]);

  // Reconcile appearance once the backend config loads: it's the source of
  // truth, so on divergence it overrides the localStorage instant-boot cache.
  // Runs on the settings identity change (load, and the update reconcile);
  // idempotent, so it settles without fighting a fresh user change.
  useEffect(() => {
    if (status !== 'ready' || !settings) return;
    const backendTheme = settings.app.theme;
    if (setMode) {
      if (backendTheme !== mode) setMode(backendTheme);
    } else if (backendTheme !== 'system' && backendTheme !== theme) {
      // The 2-way control can't represent 'system'; leave the resolved theme.
      setTheme(backendTheme);
    }
    const backendDensity = settings.app.density ?? 'comfortable';
    if (backendDensity !== density) setDensity(backendDensity);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [status, settings]);

  const backendReady = status === 'ready' && settings !== null;
  const backendDisabled = !backendReady;
  const [confirm, setConfirm] = useState<ConfirmState | null>(null);

  // While the backend config is still loading, an appearance change would apply
  // locally and then be silently reverted by the arriving snapshot — so disable
  // the controls until it settles. 'unavailable'/'error' keep them enabled:
  // there is no backend to revert or persist, so appearance stays local-only.
  const appearanceDisabled = status === 'loading';

  const app = settings?.app;
  const engine = settings?.engine;
  const updater = settings?.updater;
  const telemetry = settings?.telemetry;
  const meta = settings?.meta;
  const limits = meta?.limits;

  const defaultProfileId =
    profiles.find((p) => p.default)?.id ?? profiles[0]?.id ?? '';

  /** The inline error text for a knob, phrased from the server's own bounds. */
  const rangeError = (bounds: Bounds | undefined): string => {
    if (!bounds) return t('engine.wholeNumber');
    const params = { min: bounds.min, max: bounds.max };
    return bounds.zero === 'disables'
      ? t('engine.rangeOrZero', params)
      : t('engine.range', params);
  };

  // Dual-write: the setter props update the localStorage cache + in-memory
  // theme now; persisting the same value to the backend (when ready) keeps the
  // source of truth in step. In the standalone preview (not ready) appearance
  // stays local-only — no backend write.
  const onThemeMode = (m: ThemeMode) => {
    setMode?.(m);
    if (backendReady) void patch({ app: { theme: m } });
  };
  const onThemeResolved = (tm: ResolvedTheme) => {
    setTheme(tm);
    if (backendReady) void patch({ app: { theme: tm } });
  };
  /**
   * FR-086: switching to a more stable channel needs an explicit yes, since
   * nothing is downgraded and the next update only comes once that channel
   * catches up with the installed version. A move up applies at once.
   */
  const onChannel = (next: UpdateChannel) => {
    const current = updater?.channel ?? 'stable';
    if (next === current) return;
    const save = () => void patch({ updater: { channel: next } });
    if (CHANNEL_RANK[next] >= CHANNEL_RANK[current]) {
      save();
      return;
    }
    const name = t(CHANNEL_LABEL[next]);
    setConfirm({
      title: t('updates.lowerChannel.title', { channel: name }),
      message: t('updates.lowerChannel.message', {
        channel: name,
        current: t(CHANNEL_LABEL[current]),
      }),
      confirmLabel: t('updates.lowerChannel.confirm', { channel: name }),
      run: save,
    });
  };
  const onDensity = (d: Density) => {
    setDensity(d);
    if (backendReady) void patch({ app: { density: d } });
  };

  return (
    <>
      <MPTopBar title={t('title')} />
      <div
        className="mp-scroll"
        style={{
          flex: 1,
          overflow: 'auto',
          padding: '18px 24px',
          maxWidth: 760,
        }}
      >
        {meta?.config_unreadable && (
          <ConfigWarning path={meta.config_path} t={t} />
        )}

        <MPSectionLabel>{t('sections.appearance')}</MPSectionLabel>
        <Row
          label={t('appearance.theme.label')}
          hint={t('appearance.theme.hint')}
          disabled={appearanceDisabled}
        >
          {mode !== undefined && setMode ? (
            <MPSegmented<ThemeMode>
              value={mode}
              onChange={onThemeMode}
              aria-label={t('appearance.theme.label')}
              options={[
                {
                  value: 'light',
                  icon: 'sun',
                  label: t('appearance.theme.light'),
                },
                {
                  value: 'dark',
                  icon: 'moon',
                  label: t('appearance.theme.dark'),
                },
                { value: 'system', label: t('appearance.theme.system') },
              ]}
            />
          ) : (
            <MPSegmented<ResolvedTheme>
              value={theme}
              onChange={onThemeResolved}
              aria-label={t('appearance.theme.label')}
              options={[
                {
                  value: 'light',
                  icon: 'sun',
                  label: t('appearance.theme.light'),
                },
                {
                  value: 'dark',
                  icon: 'moon',
                  label: t('appearance.theme.dark'),
                },
              ]}
            />
          )}
        </Row>
        <Row
          label={t('appearance.density.label')}
          hint={t('appearance.density.hint')}
          disabled={appearanceDisabled}
        >
          <MPSegmented<Density>
            value={density}
            onChange={onDensity}
            aria-label={t('appearance.density.label')}
            options={[
              {
                value: 'comfortable',
                label: t('appearance.density.comfortable'),
              },
              { value: 'compact', label: t('appearance.density.compact') },
            ]}
          />
        </Row>
        {/* FR-094: the language of the whole UI. Persisted like any other
            setting; `I18nProvider` reads it back, so the app re-renders in the
            new language as soon as the optimistic write lands. */}
        <Row
          label={t('appearance.language.label')}
          hint={t('appearance.language.hint')}
          disabled={backendDisabled}
        >
          {/* Bound to the EFFECTIVE locale, not the raw stored tag: a stored
              'es-MX' renders Spanish (matched to es-ES), and a select showing
              an option that is not the stored value could never fire a change
              back to it. */}
          <MPSelect
            value={matchLocale(app?.locale) ?? locale}
            // A language picked here is final: first-run detection never
            // overrides it (FR-093).
            onChange={(e) =>
              void patch({
                app: { locale: e.target.value, locale_detected: true },
              })
            }
            aria-label={t('appearance.language.label')}
            options={SUPPORTED_LOCALES.map((l) => ({
              value: l,
              label: LOCALE_LABELS[l],
            }))}
          />
        </Row>
        {/* Honoured at startup: `lib.rs` reads `settings::launch_minimized()`
            in `setup` and minimizes the main window (there is no tray), except
            on a launch that carries a context-menu selection. */}
        <Row
          label={t('appearance.launchMinimized.label')}
          hint={t('appearance.launchMinimized.hint')}
          disabled={backendDisabled}
        >
          <MPSwitch
            checked={app?.launch_minimized ?? false}
            aria-label={t('appearance.launchMinimized.label')}
            onChange={() =>
              void patch({
                app: { launch_minimized: !(app?.launch_minimized ?? false) },
              })
            }
          />
        </Row>
        {/* FR-072: the backend reads this before every native notification
            (`notify_native`), so the switch takes effect immediately. An
            absent field means a config from before the switch: on. */}
        <Row
          label={t('appearance.nativeNotifications.label')}
          hint={t('appearance.nativeNotifications.hint')}
          disabled={backendDisabled}
        >
          <MPSwitch
            checked={app?.native_notifications ?? true}
            aria-label={t('appearance.nativeNotifications.label')}
            onChange={() =>
              void patch({
                app: {
                  native_notifications: !(app?.native_notifications ?? true),
                },
              })
            }
          />
        </Row>

        {status === 'unavailable' && <Note>{t('status.unavailable')}</Note>}
        {status === 'loading' && <Note>{t('status.loading')}</Note>}
        {status === 'error' && (
          <Note>
            {t('status.error')}{' '}
            <RetryButton
              label={t('status.retry')}
              onClick={() => void initSettings()}
            />
          </Note>
        )}

        <div style={{ height: 24 }} />
        <MPSectionLabel>{t('sections.printing')}</MPSectionLabel>
        {/* `set_default_profile` writes config.toml, so the row needs both a
            loaded profile list and a reachable settings backend. In the
            standalone preview the list is the static built-ins, and picking
            one would only bounce off a missing host. */}
        <Row
          label={t('printing.defaultProfile.label')}
          hint={t('printing.defaultProfile.hint')}
          disabled={
            backendDisabled ||
            profilesStatus !== 'ready' ||
            profiles.length === 0
          }
        >
          <MPSelect
            value={defaultProfileId}
            onChange={(e) => void setDefaultProfile(e.target.value)}
            aria-label={t('printing.defaultProfile.label')}
            options={profiles.map((p) => ({ value: p.id, label: p.name }))}
          />
        </Row>
        {profilesStatus === 'error' && (
          <Note>
            {t('printing.profilesError')}{' '}
            <RetryButton
              label={t('status.retry')}
              onClick={() => void initProfiles()}
            />
          </Note>
        )}
        <Note>{t(shellNoteKey(host))}</Note>

        <BrowserIntegrationSection />

        <div style={{ height: 24 }} />
        <MPSectionLabel>{t('sections.engine')}</MPSectionLabel>
        <EngineField
          label={t('engine.concurrency.label')}
          hint={t('engine.concurrency.hint')}
          suffix={t('engine.concurrency.suffix')}
          value={engine?.max_concurrent_conversions}
          bounds={limits?.max_concurrent_conversions}
          errorHint={rangeError(limits?.max_concurrent_conversions)}
          onCommit={(n) =>
            void patch({ engine: { max_concurrent_conversions: n } })
          }
          disabled={backendDisabled}
        />
        <EngineField
          label={t('engine.timeout.label')}
          hint={t('engine.timeout.hint')}
          suffix={t('engine.timeout.suffix')}
          value={engine?.conversion_timeout_seconds}
          bounds={limits?.conversion_timeout_seconds}
          errorHint={rangeError(limits?.conversion_timeout_seconds)}
          onCommit={(n) =>
            void patch({ engine: { conversion_timeout_seconds: n } })
          }
          disabled={backendDisabled}
        />
        <EngineField
          label={t('engine.printTimeout.label')}
          hint={t('engine.printTimeout.hint')}
          suffix={t('engine.printTimeout.suffix')}
          value={engine?.print_timeout_seconds}
          bounds={limits?.print_timeout_seconds}
          errorHint={rangeError(limits?.print_timeout_seconds)}
          onCommit={(n) => void patch({ engine: { print_timeout_seconds: n } })}
          disabled={backendDisabled}
        />
        <EngineField
          label={t('engine.maxFileSize.label')}
          hint={t('engine.maxFileSize.hint')}
          suffix={t('engine.maxFileSize.suffix')}
          value={engine?.max_file_size_mb}
          bounds={limits?.max_file_size_mb}
          errorHint={rangeError(limits?.max_file_size_mb)}
          onCommit={(n) => void patch({ engine: { max_file_size_mb: n } })}
          disabled={backendDisabled}
        />

        <div style={{ height: 24 }} />
        <MPSectionLabel>{t('sections.updates')}</MPSectionLabel>
        <Row label={t('updates.channel.label')} disabled={backendDisabled}>
          <MPSegmented<UpdateChannel>
            value={updater?.channel ?? 'stable'}
            onChange={onChannel}
            aria-label={t('updates.channel.label')}
            options={[
              { value: 'stable', label: t('updates.channel.stable') },
              { value: 'beta', label: t('updates.channel.beta') },
              { value: 'nightly', label: t('updates.channel.nightly') },
            ]}
          />
        </Row>
        <Note>{t('updates.channelNote')}</Note>
        {/* FR-080: this switch is live. `lib.rs` runs `update::run_startup_check`
            on a detached thread 30 s after launch, and the preference plus the
            24 h throttle are enforced in Rust — so the hint below states the
            frequency instead of promising a future milestone. */}
        <Row
          label={t('updates.checkOnStartup.label')}
          hint={t('updates.checkOnStartup.hint')}
          disabled={backendDisabled}
        >
          <MPSwitch
            checked={updater?.check_on_startup ?? true}
            aria-label={t('updates.checkOnStartup.label')}
            onChange={() =>
              void patch({
                updater: {
                  check_on_startup: !(updater?.check_on_startup ?? true),
                },
              })
            }
          />
        </Row>
        <Note>{t('updates.startupNote')}</Note>

        <div style={{ height: 24 }} />
        <MPSectionLabel>{t('sections.privacy')}</MPSectionLabel>
        <Row
          label={t('privacy.crashReports.label')}
          hint={t('privacy.crashReports.hint')}
          disabled={backendDisabled}
        >
          <MPSwitch
            checked={telemetry?.crash_reports ?? false}
            aria-label={t('privacy.crashReports.label')}
            onChange={() =>
              void patch({
                telemetry: {
                  crash_reports: !(telemetry?.crash_reports ?? false),
                },
              })
            }
          />
        </Row>
        <Row
          label={t('privacy.usageMetrics.label')}
          hint={t('privacy.usageMetrics.hint')}
          disabled={backendDisabled}
        >
          <MPSwitch
            checked={telemetry?.anonymous_usage_metrics ?? false}
            aria-label={t('privacy.usageMetrics.label')}
            onChange={() =>
              void patch({
                telemetry: {
                  anonymous_usage_metrics: !(
                    telemetry?.anonymous_usage_metrics ?? false
                  ),
                },
              })
            }
          />
        </Row>
        <Note>{t('privacy.note')}</Note>
      </div>
      {confirm && (
        <ConfirmDialog state={confirm} onClose={() => setConfirm(null)} />
      )}
    </>
  );
}
