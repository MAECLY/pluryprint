import { useEffect, useId, useRef, useState } from 'react';
import { Icon, type IconName } from '@/components/Icon';
import { MPAppIcon } from '@/components/Brand';
import { shortcutLabel } from '@/components/AppShell/MPAppShell';
import { ModalOverlay } from '@/components/Modal';
import { MPButton, MPCard } from '@/components/ui';
import { useModalFocus } from '@/components/useModal';
import { useT } from '@/i18n/I18nProvider';
import { cliProgram } from '@/lib/format';
import {
  appImageNameOf,
  linuxMenuKey,
  rememberedHostInfo,
  useHostPlatform,
} from '@/lib/platform';
import { findProfileIn } from '@/lib/profiles';
import { useProfilesStore } from '@/stores/profilesStore';

/**
 * First-run tour.
 *
 * Every step must describe something this build actually does: the tour is the
 * first thing a user reads, and a promise it cannot keep (a right-click entry
 * that does not exist, a Settings switch that was never shipped) is a broken
 * feature as far as they are concerned. The right-click entry is described per
 * platform — Finder Services on macOS; on Windows the installer's Explorer
 * entry, which Windows 11 keeps under "Show more options"; on Linux "Open
 * with" plus the Dolphin and Nemo actions the .deb installs, which an AppImage
 * only gets once the user integrates it — the command-line program is named
 * per platform (`pluryprint-cli` on Windows, the AppImage's own file for an
 * AppImage), and the default profile is read from the real profile list
 * instead of being retyped here.
 */
const STEPS = [
  { key: 'welcome', icon: 'maecly' },
  { key: 'printers', icon: 'printer' },
  { key: 'cli', icon: 'cmd' },
  { key: 'profiles', icon: 'sliders' },
  { key: 'ready', icon: 'check' },
] as const satisfies readonly { key: string; icon: IconName }[];

interface OnboardingFlowProps {
  onComplete: () => void;
}

export function OnboardingFlow({ onComplete }: OnboardingFlowProps) {
  const t = useT('home');
  const profiles = useProfilesStore((s) => s.profiles);
  const [step, setStep] = useState(0);
  const isLast = step === STEPS.length - 1;
  // Non-null: `step` starts at 0 and the Back/Continue handlers below keep it
  // within [0, STEPS.length - 1].
  const cur = STEPS[step]!;
  const host = useHostPlatform();
  const hostInfo = rememberedHostInfo();
  const program = cliProgram(host.os, {
    linuxPackage: host.linuxPackage,
    appImage: appImageNameOf(hostInfo),
  });
  const params = {
    name: findProfileIn(profiles, null).name,
    command: `${program} print --files "*.pdf"`,
    shortcut: shortcutLabel('newJob'),
    menu: t(linuxMenuKey(host.linuxPackage)),
  };
  // FR-001: the right-click entry and "Save as PDF" are platform-specific, so
  // those two steps have a macOS and a Linux wording.
  const perPlatform = cur.key === 'ready' || cur.key === 'printers';
  const bodyKey =
    perPlatform && host.os === 'macos'
      ? (`onboarding.${cur.key}.bodyMac` as const)
      : perPlatform && host.os === 'linux'
        ? (`onboarding.${cur.key}.bodyLinux` as const)
        : (`onboarding.${cur.key}.body` as const);
  const dialogRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const bodyId = useId();

  // FR-111 / NFR-041 / NFR-044: the tour is a real modal — focus moves in, Tab
  // stays inside, Escape skips, the app behind it is inert and its shortcuts
  // are suspended until the tour is done.
  useModalFocus(dialogRef, { onEscape: onComplete });

  // On open and on every step, put focus on the primary action (the last
  // button: Continue / Get started) so Enter walks the tour.
  useEffect(() => {
    const buttons = dialogRef.current?.querySelectorAll('button');
    buttons?.[buttons.length - 1]?.focus();
  }, [step]);

  return (
    <ModalOverlay
      style={{
        background: 'rgba(15, 14, 11, 0.45)',
        backdropFilter: 'blur(4px)',
        animation: 'mp-fadein .3s ease',
      }}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={bodyId}
        style={{ width: 520, maxWidth: '90vw' }}
      >
        <MPCard
          padding={28}
          style={{
            boxShadow:
              '0 8px 24px rgba(0,0,0,0.18), 0 0 0 1px var(--mp-border)',
          }}
        >
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 14,
              marginBottom: 18,
            }}
          >
            {step === 0 ? (
              <MPAppIcon size={48} />
            ) : (
              <div
                style={{
                  width: 48,
                  height: 48,
                  borderRadius: 10,
                  background: 'var(--mp-accentSoft)',
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                }}
              >
                <Icon name={cur.icon} size={22} color="var(--mp-accent)" />
              </div>
            )}
            <div style={{ flex: 1 }}>
              <div
                id={titleId}
                style={{
                  fontSize: 16,
                  fontWeight: 600,
                  letterSpacing: '-0.01em',
                }}
              >
                {t(`onboarding.${cur.key}.title`)}
              </div>
              <div
                id={bodyId}
                style={{
                  fontSize: 12.5,
                  color: 'var(--mp-textMuted)',
                  marginTop: 4,
                  lineHeight: 1.5,
                }}
              >
                {t(bodyKey, params)}
              </div>
            </div>
          </div>

          {/* Progress dots — decorative, so the step is also stated in text for
            anyone who cannot see them. */}
          <div
            role="status"
            aria-label={t('onboarding.progress', {
              step: step + 1,
              total: STEPS.length,
            })}
            style={{
              display: 'flex',
              justifyContent: 'center',
              gap: 6,
              marginBottom: 20,
            }}
          >
            {STEPS.map((s, i) => (
              <span
                key={s.key}
                style={{
                  width: i === step ? 22 : 6,
                  height: 6,
                  borderRadius: 3,
                  background:
                    i === step
                      ? 'var(--mp-accent)'
                      : i < step
                        ? 'var(--mp-borderStrong)'
                        : 'var(--mp-border)',
                  transition: 'all .25s',
                }}
              />
            ))}
          </div>

          <div
            style={{
              display: 'flex',
              justifyContent: 'space-between',
              alignItems: 'center',
              gap: 10,
            }}
          >
            <MPButton variant="ghost" size="sm" onClick={() => onComplete()}>
              {t('onboarding.skip')}
            </MPButton>
            <div style={{ display: 'flex', gap: 8 }}>
              {step > 0 && (
                <MPButton
                  variant="secondary"
                  icon="arrowl"
                  size="sm"
                  onClick={() => setStep(step - 1)}
                >
                  {t('onboarding.back')}
                </MPButton>
              )}
              <MPButton
                variant="primary"
                iconRight={isLast ? 'check' : 'arrowr'}
                size="sm"
                onClick={() => {
                  if (isLast) onComplete();
                  else setStep(step + 1);
                }}
              >
                {isLast ? t('onboarding.start') : t('onboarding.continue')}
              </MPButton>
            </div>
          </div>
        </MPCard>
      </div>
    </ModalOverlay>
  );
}
