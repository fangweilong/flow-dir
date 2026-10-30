import { useState } from 'react'
import type { Key, Translate } from '../../i18n'
import type { LogLevel, Settings } from '../../persist'
import { diagnosticsExport, logDirPath, pickFolder } from '../../api'
import { LOG_LEVELS, ipcErrorText } from './SettingsShared'

export type DiagnosticsSectionProps = {
  settings: Settings
  onChange: (next: Settings) => void
  onOpenLocation: (path: string) => void
  t: Translate
  visible: boolean
}

export function DiagnosticsSection({ settings, onChange, onOpenLocation, t, visible }: DiagnosticsSectionProps) {
  const [exporting, setExporting] = useState(false)
  const [exportNote, setExportNote] = useState('')

  if (!visible) return null

  return (
    <section>
      <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">
        {t('settings.diagnostics')}
      </h2>
      <div className="space-y-2 rounded-lg border border-line px-3 py-2" data-diagnostics>
        <div className="flex items-center justify-between gap-2">
          <div className="min-w-0">
            <div className="text-[0.71875rem] text-fg">{t('settings.logLevel')}</div>
            <div className="text-[0.625rem] text-muted-2">{t('settings.logLevelHint')}</div>
          </div>
          <select
            value={settings.logLevel}
            onChange={(ev) => onChange({ ...settings, logLevel: ev.target.value as LogLevel })}
            data-log-level
            className="h-6 shrink-0 rounded border border-line bg-surface-2 px-1 text-[0.6875rem] text-fg outline-none focus:border-blue-400"
          >
            {LOG_LEVELS.map((level) => (
              <option key={level} value={level}>
                {t(('settings.logLevel.' + level) as Key)}
              </option>
            ))}
          </select>
        </div>
        <div className="flex items-center gap-2 border-t border-line pt-2">
          <button
            type="button"
            onClick={() =>
              void logDirPath()
                .then((dir) => (dir ? onOpenLocation(dir) : undefined))
                .catch(() => undefined)
            }
            className="shrink-0 rounded border border-line px-2 py-0.5 text-[0.6875rem] text-fg hover:bg-surface-3"
          >
            {t('settings.logOpen')}
          </button>
          <button
            type="button"
            disabled={exporting}
            onClick={() => {
              setExporting(true)
              setExportNote('')
              void pickFolder(t('settings.logExportPick'), '')
                .then((dir) => (dir ? diagnosticsExport(dir) : null))
                .then((path) => {
                  if (path) setExportNote(t('settings.logExported', { path }))
                })
                .catch((err: unknown) => {
                  setExportNote(`${t('settings.logExportFailed')}: ${ipcErrorText(err)}`)
                })
                .finally(() => setExporting(false))
            }}
            className="shrink-0 rounded border border-line px-2 py-0.5 text-[0.6875rem] text-fg hover:bg-surface-3 disabled:opacity-40"
          >
            {t('settings.logExport')}
          </button>
          <span
            className="min-w-0 flex-1 truncate text-[0.625rem] text-muted-2"
            title={exportNote}
          >
            {exportNote}
          </span>
        </div>
      </div>
    </section>
  )
}
