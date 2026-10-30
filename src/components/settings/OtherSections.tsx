import type { Translate } from '../../i18n'
import type { Settings } from '../../persist'
import { restartApp, settingsSave } from '../../api'
import { ToggleRow } from './SettingsShared'

export function ColumnsSection({
  settings,
  onChange,
  t,
  visible,
}: {
  settings: Settings
  onChange: (next: Settings) => void
  t: Translate
  visible: boolean
}) {
  if (!visible) return null

  return (
    <section>
      <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">{t('settings.columns')}</h2>
      <div className="space-y-1.5" data-columns>
        <div className="text-[0.65625rem] text-muted-2">{t('settings.columnsHint')}</div>
        <ToggleRow
          label={t('list.size')}
          value={settings.listColSize}
          onChange={(value) => onChange({ ...settings, listColSize: value })}
        />
        <ToggleRow
          label={t('list.type')}
          value={settings.listColType}
          onChange={(value) => onChange({ ...settings, listColType: value })}
        />
        <ToggleRow
          label={t('list.mtime')}
          value={settings.listColTime}
          onChange={(value) => onChange({ ...settings, listColTime: value })}
        />
      </div>
    </section>
  )
}

export function GraphicsSection({
  settings,
  onChange,
  t,
  visible,
}: {
  settings: Settings
  onChange: (next: Settings) => void
  t: Translate
  visible: boolean
}) {
  if (!visible) return null

  return (
    <section>
      <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">{t('settings.graphics')}</h2>
      <div data-gpu-section>
        <ToggleRow
          label={t('settings.gpuAccel')}
          hint={t('settings.gpuAccelHint')}
          value={settings.gpuAccel}
          onChange={(value) => onChange({ ...settings, gpuAccel: value })}
        />
        <div className="mt-1.5 flex items-center justify-between gap-2 rounded-lg border border-line px-3 py-2">
          <span className="text-[0.65625rem] text-muted-2">{t('settings.gpuRestartHint')}</span>
          <button
            type="button"
            onClick={() => {
              void settingsSave(settings)
                .catch(() => undefined)
                .then(() => restartApp().catch(() => undefined))
            }}
            className="shrink-0 rounded-md border border-line px-2 py-1 text-[0.6875rem] text-fg hover:bg-surface-3"
          >
            {t('settings.restartNow')}
          </button>
        </div>
        <div className="mt-3">
          <ToggleRow
            label={t('settings.reclaimMemory')}
            hint={t('settings.reclaimMemoryHint')}
            value={settings.reclaimMemory}
            onChange={(value) => onChange({ ...settings, reclaimMemory: value })}
          />
        </div>
      </div>
    </section>
  )
}

export function ConfigSection({
  configDir,
  onOpenLocation,
  t,
  visible,
}: {
  configDir: string
  onOpenLocation: (path: string) => void
  t: Translate
  visible: boolean
}) {
  if (!visible) return null

  return (
    <section>
      <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">{t('settings.configDir')}</h2>
      <div className="flex items-center gap-2 rounded-lg border border-line px-3 py-2">
        <span className="min-w-0 flex-1 truncate font-mono text-[0.6875rem] text-muted" title={configDir} data-config-dir>
          {configDir || '…'}
        </span>
        <button
          type="button"
          disabled={!configDir}
          onClick={() => configDir && onOpenLocation(configDir)}
          className="shrink-0 rounded-md border border-line px-2 py-1 text-[0.6875rem] text-fg hover:bg-surface-3 disabled:opacity-40"
        >
          {t('settings.configOpen')}
        </button>
      </div>
    </section>
  )
}
