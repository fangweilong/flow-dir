import { useState } from 'react'
import type { Translate } from '../../i18n'
import type { Settings } from '../../persist'
import { formatBinding } from '../../shortcuts'
import type { ShortcutId } from '../../shortcuts'

export type ExternalPreviewSectionProps = {
  settings: Settings
  keys: Record<ShortcutId, string>
  onChange: (next: Settings) => void
  t: Translate
  visible: boolean
}

export function ExternalPreviewSection({ settings, keys, onChange, t, visible }: ExternalPreviewSectionProps) {
  const [externalExt, setExternalExt] = useState('')
  const [externalCommand, setExternalCommand] = useState('')

  const addExternal = () => {
    const ext = externalExt.trim().replace(/^\.+/, '').toLowerCase()
    const command = externalCommand.trim()
    if (!ext || !command) return
    const rest = settings.externalPreview.filter((rule) => rule.ext !== ext)
    onChange({ ...settings, externalPreview: [...rest, { ext, command }] })
    setExternalExt('')
    setExternalCommand('')
  }

  if (!visible) return null

  return (
    <section>
      <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">
        {t('settings.externalPreview')}
      </h2>
      <div className="space-y-1.5 rounded-lg border border-line px-3 py-2" data-external-preview>
        <div className="pb-1 text-[0.65625rem] text-muted-2">{t('settings.externalPreviewHint')}</div>
        <div className="pb-1 text-[0.65625rem] text-muted-2">
          {t('settings.externalKey', { keys: formatBinding(keys.externalPreview) })}
        </div>
        {settings.externalPreview.map((rule) => (
          <div key={rule.ext} className="flex items-center justify-between gap-2 text-[0.71875rem]">
            <span className="min-w-0">
              <span className="font-mono text-fg">.{rule.ext}</span>
              <span className="ml-2 truncate font-mono text-[0.65625rem] text-muted-2" title={rule.command}>
                {rule.command}
              </span>
            </span>
            <button
              type="button"
              onClick={() =>
                onChange({
                  ...settings,
                  externalPreview: settings.externalPreview.filter((entry) => entry.ext !== rule.ext),
                })
              }
              className="shrink-0 rounded px-1.5 py-0.5 text-[0.6875rem] text-muted hover:bg-surface-3 hover:text-red-500"
            >
              {t('settings.placeDelete')}
            </button>
          </div>
        ))}
        <div className="flex items-center gap-1.5 border-t border-line pt-2">
          <input
            value={externalExt}
            onChange={(ev) => setExternalExt(ev.target.value)}
            placeholder={t('settings.externalExtPh')}
            data-external-ext
            className="h-6 w-[4.5rem] shrink-0 rounded border border-line bg-surface-2 px-1.5 text-[0.6875rem] outline-none placeholder:text-muted-2 focus:border-blue-400 focus:bg-surface"
          />
          <input
            value={externalCommand}
            onChange={(ev) => setExternalCommand(ev.target.value)}
            onKeyDown={(ev) => {
              if (ev.key === 'Enter') addExternal()
            }}
            placeholder={t('settings.externalCommandPh')}
            title={t('settings.externalCommand')}
            data-external-command
            className="h-6 min-w-0 flex-1 rounded border border-line bg-surface-2 px-1.5 text-[0.6875rem] outline-none placeholder:text-muted-2 focus:border-blue-400 focus:bg-surface"
          />
          <button
            type="button"
            onClick={addExternal}
            disabled={!externalExt.trim() || !externalCommand.trim()}
            className="h-6 shrink-0 rounded bg-blue-600 px-2 text-[0.6875rem] text-white hover:bg-blue-500 disabled:opacity-40"
          >
            {t('settings.placeAdd')}
          </button>
        </div>
      </div>
      <div className="mt-2 space-y-2 rounded-lg border border-line px-3 py-2" data-preview-limits>
        <div className="flex items-center justify-between gap-2">
          <div className="min-w-0">
            <div className="text-[0.71875rem] text-fg">{t('settings.previewMaxFile')}</div>
            <div className="text-[0.625rem] text-muted-2">{t('settings.previewMaxFileHint')}</div>
          </div>
          <input
            type="number"
            min={1}
            max={4096}
            value={settings.previewMaxFileMb}
            onChange={(ev) => {
              const next = Number(ev.target.value)
              if (!Number.isFinite(next)) return
              onChange({ ...settings, previewMaxFileMb: Math.min(4096, Math.max(1, Math.round(next))) })
            }}
            data-preview-max-file
            className="h-6 w-16 shrink-0 rounded border border-line bg-surface-2 px-1 text-right text-[0.6875rem] tabular text-fg outline-none focus:border-blue-400"
          />
        </div>
        <div className="flex items-center justify-between gap-2">
          <div className="min-w-0">
            <div className="text-[0.71875rem] text-fg">{t('settings.previewMaxDimension')}</div>
            <div className="text-[0.625rem] text-muted-2">{t('settings.previewMaxDimensionHint')}</div>
          </div>
          <input
            type="number"
            min={256}
            max={16384}
            step={64}
            value={settings.previewMaxDimension}
            onChange={(ev) => {
              const next = Number(ev.target.value)
              if (!Number.isFinite(next)) return
              onChange({
                ...settings,
                previewMaxDimension: Math.min(16384, Math.max(256, Math.round(next))),
              })
            }}
            data-preview-max-dimension
            className="h-6 w-20 shrink-0 rounded border border-line bg-surface-2 px-1 text-right text-[0.6875rem] tabular text-fg outline-none focus:border-blue-400"
          />
        </div>
        <div className="flex items-center justify-between gap-2">
          <div className="min-w-0">
            <div className="text-[0.71875rem] text-fg">{t('settings.previewTextKb')}</div>
            <div className="text-[0.625rem] text-muted-2">{t('settings.previewTextKbHint')}</div>
          </div>
          <input
            type="number"
            min={1}
            max={1024}
            value={settings.previewTextKb}
            onChange={(ev) => {
              const next = Number(ev.target.value)
              if (!Number.isFinite(next)) return
              onChange({ ...settings, previewTextKb: Math.min(1024, Math.max(1, Math.round(next))) })
            }}
            data-preview-text-kb
            className="h-6 w-16 shrink-0 rounded border border-line bg-surface-2 px-1 text-right text-[0.6875rem] tabular text-fg outline-none focus:border-blue-400"
          />
        </div>
      </div>
    </section>
  )
}
