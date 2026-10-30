import type { Translate } from '../../i18n'
import type { AppInfo } from '../../types'

export type AboutSectionProps = {
  info: AppInfo | null
  t: Translate
  visible: boolean
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
        <div className="mt-2 border-t border-line pt-2">
          <div className="pb-1 text-[0.65625rem] text-muted-2">{t('settings.dependencies')}</div>
          {(['rust', 'npm'] as const).map((kind) => {
            const rows = (info?.dependencies ?? []).filter((dep) => dep.kind === kind)
            if (!rows.length) return null
            return (
              <div key={kind} className="mb-2 last:mb-0">
                <div className="pb-0.5 text-[0.59375rem] font-semibold uppercase tracking-wider text-muted-2">
                  {kind}
                </div>
                <div className="grid grid-cols-[minmax(0,1fr)_4.5rem_8rem] items-baseline gap-x-3 gap-y-1">
                  {rows.map((dep) => (
                    <div key={dep.name} className="contents">
                      <span className="truncate font-mono text-[0.6875rem] text-fg" title={dep.name}>
                        {dep.name}
                      </span>
                      <span className="text-right font-mono text-[0.6875rem] tabular text-muted">
                        {dep.version}
                      </span>
                      <span className="truncate text-right text-[0.65625rem] text-muted-2" title={dep.license}>
                        {dep.license}
                      </span>
                    </div>
                  ))}
                </div>
              </div>
            )
          })}
        </div>
      </div>
    </section>
  )
}
