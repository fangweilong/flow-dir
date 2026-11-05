import { openUrl } from '../../api'
import type { Translate } from '../../i18n'
import type { AppInfo, Dependency } from '../../types'

export type AboutSectionProps = {
  info: AppInfo | null
  t: Translate
  visible: boolean
}

/** Project destinations are build metadata, not user input, but the backend still
 * validates the scheme before handing a URL to the shell. */
function LinkRow({ label, url }: { label: string; url: string | undefined }) {
  return (
    <button
      type="button"
      disabled={!url}
      title={url}
      onClick={() => url && void openUrl(url).catch(() => undefined)}
      className="flex w-full items-center justify-between gap-3 rounded px-1 py-1 text-left hover:bg-surface-3 disabled:opacity-40"
    >
      <span className="shrink-0 text-[0.71875rem] text-fg">{label}</span>
      <span className="flex min-w-0 items-center gap-1 text-[0.65625rem] text-muted-2">
        <span className="truncate">{url ?? '…'}</span>
        <svg aria-hidden viewBox="0 0 16 16" className="h-3 w-3 shrink-0" fill="none" stroke="currentColor" strokeWidth="1.4">
          <path d="M6.5 3.5h-3v9h9v-3" strokeLinecap="round" strokeLinejoin="round" />
          <path d="M9.5 3.5h3v3M12.5 3.5 7.5 8.5" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </span>
    </button>
  )
}

export function AboutSection({ info, t, visible }: AboutSectionProps) {
  if (!visible) return null

  return (
    <section>
      <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">{t('settings.about')}</h2>
      <div className="rounded-lg border border-line px-3 py-2 text-[0.71875rem]">
        <div className="flex justify-between py-0.5">
          <span className="text-muted">{t('settings.version')}</span>
          <span className="font-mono text-fg">{info?.version ?? '…'}</span>
        </div>
        <div className="mt-2 space-y-0.5 border-t border-line pt-1.5">
          <LinkRow label={t('settings.repository')} url={info?.repositoryUrl} />
          <LinkRow label={t('settings.profile')} url={info?.profileUrl} />
        </div>
      </div>
    </section>
  )
}

/**
 * `MIT OR Apache-2.0` is accurate SPDX but reads as undecided. Spell the choice out:
 * either license may be used, while `AND` means both apply.
 */
function licenseLabel(raw: string, t: Translate): { text: string; raw: string } {
  const cleaned = raw.trim()
  if (!cleaned || cleaned === '?' || /^(see|not installed)/i.test(cleaned)) {
    return { text: t('settings.licenseUnknown'), raw: cleaned }
  }
  const normalized = cleaned.replace(/\s*\/\s*/g, ' OR ')
  const either = normalized.split(/\s+OR\s+/i).map((part) => part.replace(/[()]/g, '').trim()).filter(Boolean)
  if (either.length > 1) {
    return { text: t('settings.licenseEither', { list: either.join(' / ') }), raw: cleaned }
  }
  const both = normalized.split(/\s+AND\s+/i).map((part) => part.replace(/[()]/g, '').trim()).filter(Boolean)
  if (both.length > 1) {
    return { text: t('settings.licenseBoth', { list: both.join(' + ') }), raw: cleaned }
  }
  return { text: cleaned, raw: cleaned }
}

function DependencyRow({ dep, t }: { dep: Dependency; t: Translate }) {
  const license = licenseLabel(dep.license, t)
  return (
    <div className="contents">
      <button
        type="button"
        title={dep.repository || dep.name}
        onClick={() => dep.repository && void openUrl(dep.repository).catch(() => undefined)}
        className="min-w-0 truncate text-left font-mono text-[0.6875rem] text-blue-600 hover:underline dark:text-blue-400"
      >
        {dep.name}
      </button>
      <span className="text-right font-mono text-[0.6875rem] tabular text-muted">{dep.version}</span>
      <span
        className="truncate text-right text-[0.65625rem] text-muted-2"
        title={/( OR | AND |\/)/.test(license.raw) ? `SPDX: ${license.raw}` : undefined}
      >
        {license.text}
      </span>
    </div>
  )
}

export function DependenciesSection({
  info,
  t,
  visible,
}: {
  info: AppInfo | null
  t: Translate
  visible: boolean
}) {
  if (!visible) return null

  return (
    <section>
      <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">{t('settings.dependencies')}</h2>
      <div className="rounded-lg border border-line px-3 py-2 text-[0.71875rem]">
        <div className="pb-2 text-[0.65625rem] text-muted-2">{t('settings.dependenciesHint')}</div>
        {(['rust', 'npm'] as const).map((kind) => {
          const rows = (info?.dependencies ?? []).filter((dep) => dep.kind === kind)
          if (!rows.length) return null
          return (
            <div key={kind} className="mb-2 last:mb-0">
              <div className="pb-0.5 text-[0.59375rem] font-semibold uppercase tracking-wider text-muted-2">
                {kind}
              </div>
              <div className="grid grid-cols-[minmax(0,1fr)_4.5rem_10.5rem] items-baseline gap-x-3 gap-y-1">
                {rows.map((dep) => (
                  <DependencyRow key={dep.name} dep={dep} t={t} />
                ))}
              </div>
            </div>
          )
        })}
      </div>
    </section>
  )
}
