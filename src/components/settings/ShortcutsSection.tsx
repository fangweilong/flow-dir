import { useEffect, useState } from 'react'
import type { Translate } from '../../i18n'
import type { Settings } from '../../persist'
import {
  DEFAULT_BINDINGS,
  SHORTCUTS,
  bindingFromEvent,
  fixedBindingName,
  formatBinding,
  sameBinding,
} from '../../shortcuts'
import type { ShortcutId } from '../../shortcuts'

export type ShortcutsSectionProps = {
  settings: Settings
  keys: Record<ShortcutId, string>
  onChange: (next: Settings) => void
  t: Translate
  visible: boolean
}

export function ShortcutsSection({ settings, keys, onChange, t, visible }: ShortcutsSectionProps) {
  const [capturing, setCapturing] = useState<ShortcutId | null>(null)
  const [conflict, setConflict] = useState<string | null>(null)

  useEffect(() => {
    if (!capturing) return
    const onKey = (ev: KeyboardEvent) => {
      ev.preventDefault()
      ev.stopPropagation()
      if (ev.key === 'Escape') {
        setCapturing(null)
        setConflict(null)
        return
      }
      const binding = bindingFromEvent(ev)
      if (!binding) return
      const fixed = fixedBindingName(binding, t)
      if (fixed) {
        setConflict(t('settings.shortcutConflict', { name: fixed }))
        return
      }
      const clash = SHORTCUTS.find((entry) => entry.id !== capturing && sameBinding(keys[entry.id], binding))
      if (clash) {
        setConflict(t('settings.shortcutConflict', { name: t(clash.label) }))
        return
      }
      const rest = settings.shortcuts.filter((entry) => entry.id !== capturing)
      onChange({
        ...settings,
        shortcuts: binding === DEFAULT_BINDINGS[capturing] ? rest : [...rest, { id: capturing, keys: binding }],
      })
      setCapturing(null)
      setConflict(null)
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [capturing, keys, settings, onChange, t])

  if (!visible) return null

  return (
    <section>
      <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">{t('settings.shortcuts')}</h2>
      <div className="space-y-2" data-shortcuts>
        <div className="text-[0.65625rem] text-muted-2">{t('settings.shortcutsHint')}</div>
        {(['app', 'pane'] as const).map((scope) => (
          <div key={scope} className="overflow-hidden rounded-lg border border-line">
            <div className="border-b border-line bg-surface-2 px-3 py-1 text-[0.625rem] font-semibold uppercase tracking-wider text-muted-2">
              {scope === 'app' ? t('settings.shortcutApp') : t('settings.shortcutPane')}
            </div>
            {SHORTCUTS.filter((shortcut) => shortcut.scope === scope).map((shortcut) => {
              const overridden = !sameBinding(keys[shortcut.id], DEFAULT_BINDINGS[shortcut.id])
              return (
                <div
                  key={shortcut.id}
                  className="flex items-center justify-between gap-2 border-b border-line px-3 py-1.5 text-[0.71875rem] last:border-b-0"
                >
                  <span className="min-w-0 truncate text-fg">{t(shortcut.label)}</span>
                  <span className="flex shrink-0 items-center gap-1">
                    <button
                      type="button"
                      onClick={() => {
                        setCapturing(shortcut.id)
                        setConflict(null)
                      }}
                      data-shortcut={shortcut.id}
                      className={`min-w-[5.75rem] rounded border px-2 py-0.5 font-mono text-[0.6875rem] ${
                        capturing === shortcut.id
                          ? 'border-blue-400 bg-blue-500/10 text-blue-500'
                          : 'border-line bg-surface-2 text-fg hover:bg-surface-3'
                      }`}
                    >
                      {capturing === shortcut.id
                        ? t('settings.shortcutCapturing')
                        : formatBinding(keys[shortcut.id])}
                    </button>
                    {overridden && (
                      <button
                        type="button"
                        onClick={() =>
                          onChange({
                            ...settings,
                            shortcuts: settings.shortcuts.filter((entry) => entry.id !== shortcut.id),
                          })
                        }
                        className="rounded px-1 py-0.5 text-[0.65625rem] text-muted-2 hover:bg-surface-3 hover:text-fg"
                      >
                        {t('settings.shortcutReset')}
                      </button>
                    )}
                  </span>
                </div>
              )
            })}
          </div>
        ))}
        <div className="flex items-center justify-between gap-2 pt-0.5">
          <span className="min-h-[0.875rem] text-[0.65625rem] text-red-500">{conflict ?? ''}</span>
          <button
            type="button"
            onClick={() => onChange({ ...settings, shortcuts: [] })}
            disabled={!settings.shortcuts.length}
            className="shrink-0 rounded border border-line px-2 py-0.5 text-[0.6875rem] text-fg hover:bg-surface-3 disabled:opacity-40"
          >
            {t('settings.shortcutResetAll')}
          </button>
        </div>
      </div>
    </section>
  )
}
