import { useState } from 'react'
import { NumberField } from '../NumberField'
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
  /** Extension of the rule being edited; `null` while the form adds a new one. */
  const [editingExt, setEditingExt] = useState<string | null>(null)

  const resetDraft = () => {
    setExternalExt('')
    setExternalCommand('')
    setEditingExt(null)
  }

  const commitExternal = () => {
    const ext = externalExt.trim().replace(/^\.+/, '').toLowerCase()
    const command = externalCommand.trim()
    if (!ext || !command) return
    const rules = settings.externalPreview
    if (editingExt !== null && rules.some((rule) => rule.ext === editingExt)) {
      // Keep the edited row in place; a rule already using the new extension is dropped,
      // so the list still maps one command per extension.
      const next = rules
        .filter((rule) => rule.ext !== ext || rule.ext === editingExt)
        .map((rule) => (rule.ext === editingExt ? { ext, command } : rule))
      onChange({ ...settings, externalPreview: next })
    } else {
      const rest = rules.filter((rule) => rule.ext !== ext)
      onChange({ ...settings, externalPreview: [...rest, { ext, command }] })
    }
    resetDraft()
  }

  const startEdit = (ext: string, command: string) => {
    setEditingExt(ext)
    setExternalExt(ext)
    setExternalCommand(command)
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
          <div
            key={rule.ext}
            data-external-editing={rule.ext === editingExt ? '1' : undefined}
            className={`flex items-center justify-between gap-2 rounded px-1 text-[0.71875rem] ${
              rule.ext === editingExt ? 'bg-surface-3' : ''
            }`}
          >
            <span className="min-w-0">
              <span className="font-mono text-fg">.{rule.ext}</span>
              <span className="ml-2 truncate font-mono text-[0.65625rem] text-muted-2" title={rule.command}>
                {rule.command}
              </span>
            </span>
            <span className="flex shrink-0 items-center gap-1">
              <button
                type="button"
                onClick={() => startEdit(rule.ext, rule.command)}
                className="rounded px-1.5 py-0.5 text-[0.6875rem] text-muted hover:bg-surface-3 hover:text-accent"
              >
                {t('settings.placeEdit')}
              </button>
              <button
                type="button"
                onClick={() => {
                  if (rule.ext === editingExt) resetDraft()
                  onChange({
                    ...settings,
                    externalPreview: settings.externalPreview.filter((entry) => entry.ext !== rule.ext),
                  })
                }}
                className="rounded px-1.5 py-0.5 text-[0.6875rem] text-muted hover:bg-surface-3 hover:text-red-500"
              >
                {t('settings.placeDelete')}
              </button>
            </span>
          </div>
        ))}
        <div className="flex items-center gap-1.5 border-t border-line pt-2">
          <input
            value={externalExt}
            onChange={(ev) => setExternalExt(ev.target.value)}
            placeholder={t('settings.externalExtPh')}
            data-external-ext
            className="h-6 w-[4.5rem] shrink-0 rounded border border-line bg-surface-2 px-1.5 text-[0.6875rem] outline-none placeholder:text-muted-2 focus:border-accent-soft focus:bg-surface"
          />
          <input
            value={externalCommand}
            onChange={(ev) => setExternalCommand(ev.target.value)}
            onKeyDown={(ev) => {
              if (ev.key === 'Enter') commitExternal()
              if (ev.key === 'Escape' && editingExt !== null) resetDraft()
            }}
            placeholder={t('settings.externalCommandPh')}
            title={t('settings.externalCommand')}
            data-external-command
            className="h-6 min-w-0 flex-1 rounded border border-line bg-surface-2 px-1.5 text-[0.6875rem] outline-none placeholder:text-muted-2 focus:border-accent-soft focus:bg-surface"
          />
          <button
            type="button"
            onClick={commitExternal}
            disabled={!externalExt.trim() || !externalCommand.trim()}
            className="h-6 shrink-0 rounded bg-accent-strong px-2 text-[0.6875rem] text-white hover:bg-accent disabled:opacity-40"
          >
            {editingExt !== null ? t('settings.placeSave') : t('settings.placeAdd')}
          </button>
          {editingExt !== null && (
            <button
              type="button"
              onClick={resetDraft}
              className="h-6 shrink-0 rounded border border-line px-2 text-[0.6875rem] text-fg hover:bg-surface-3"
            >
              {t('settings.placeCancel')}
            </button>
          )}
        </div>
      </div>
      <div className="mt-2 space-y-2 rounded-lg border border-line px-3 py-2" data-preview-limits>
        <div className="flex items-center justify-between gap-2">
          <div className="min-w-0">
            <div className="text-[0.71875rem] text-fg">{t('settings.previewMaxFile')}</div>
            <div className="text-[0.625rem] text-muted-2">{t('settings.previewMaxFileHint')}</div>
          </div>
          <NumberField
            min={1}
            max={4096}
            value={settings.previewMaxFileMb}
            onChange={(value) => onChange({ ...settings, previewMaxFileMb: value })}
            data-preview-max-file
            className="h-6 w-16 shrink-0 rounded border border-line bg-surface-2 px-1 text-right text-[0.6875rem] tabular text-fg outline-none focus:border-accent-soft"
          />
        </div>
        <div className="flex items-center justify-between gap-2">
          <div className="min-w-0">
            <div className="text-[0.71875rem] text-fg">{t('settings.previewMaxDimension')}</div>
            <div className="text-[0.625rem] text-muted-2">{t('settings.previewMaxDimensionHint')}</div>
          </div>
          <NumberField
            min={256}
            max={16384}
            step={64}
            value={settings.previewMaxDimension}
            onChange={(value) => onChange({ ...settings, previewMaxDimension: value })}
            data-preview-max-dimension
            className="h-6 w-20 shrink-0 rounded border border-line bg-surface-2 px-1 text-right text-[0.6875rem] tabular text-fg outline-none focus:border-accent-soft"
          />
        </div>
        <div className="flex items-center justify-between gap-2">
          <div className="min-w-0">
            <div className="text-[0.71875rem] text-fg">{t('settings.previewTextKb')}</div>
            <div className="text-[0.625rem] text-muted-2">{t('settings.previewTextKbHint')}</div>
          </div>
          <NumberField
            min={1}
            max={4096}
            value={settings.previewTextKb}
            onChange={(value) => onChange({ ...settings, previewTextKb: value })}
            data-preview-text-kb
            className="h-6 w-16 shrink-0 rounded border border-line bg-surface-2 px-1 text-right text-[0.6875rem] tabular text-fg outline-none focus:border-accent-soft"
          />
        </div>
      </div>
    </section>
  )
}
