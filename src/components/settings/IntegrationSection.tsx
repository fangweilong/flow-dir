import { useEffect, useState } from 'react'
import type { Translate } from '../../i18n'
import type { Settings } from '../../persist'
import { bindingFromEvent, formatBinding } from '../../shortcuts'
import { GLOBAL_HOTKEY_KEY, ToggleRow } from './SettingsShared'

export type IntegrationSectionProps = {
  settings: Settings
  onChange: (next: Settings) => void
  t: Translate
  visible: boolean
}

export function IntegrationSection({ settings, onChange, t, visible }: IntegrationSectionProps) {
  const [hotkeyCapturing, setHotkeyCapturing] = useState(false)
  const [hotkeyError, setHotkeyError] = useState<string | null>(null)

  useEffect(() => {
    if (!hotkeyCapturing) return
    const onKey = (ev: KeyboardEvent) => {
      ev.preventDefault()
      ev.stopPropagation()
      if (ev.key === 'Escape') {
        setHotkeyCapturing(false)
        return
      }
      const binding = bindingFromEvent(ev)
      if (!binding) return
      const parts = binding.split('+')
      const key = parts.pop() ?? ''
      const modifier = parts.some((part) => part === 'ctrl' || part === 'alt' || part === 'shift')
      if (!modifier || !GLOBAL_HOTKEY_KEY.test(key)) {
        setHotkeyError(t('settings.hotkeyUnsupported'))
        return
      }
      setHotkeyError(null)
      setHotkeyCapturing(false)
      onChange({ ...settings, hotkeyEnabled: true, globalHotkey: binding })
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [hotkeyCapturing, settings, onChange, t])

  if (!visible) return null

  return (
    <section>
      <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">{t('settings.integration')}</h2>
      <div className="space-y-2" data-integration>
        <ToggleRow
          label={t('settings.hotkeyEnable')}
          hint={t('settings.hotkeyHint')}
          value={settings.hotkeyEnabled}
          onChange={(value) => onChange({ ...settings, hotkeyEnabled: value })}
        />
        <div className="flex items-center justify-between gap-2 rounded-lg border border-line px-3 py-2">
          <div className="min-w-0">
            <div className="text-[0.71875rem] text-fg">{t('settings.hotkey')}</div>
            <div className="text-[0.625rem] text-muted-2">{t('settings.hotkeyDefault')}</div>
          </div>
          <button
            type="button"
            data-hotkey-capture
            onClick={() => {
              setHotkeyError(null)
              setHotkeyCapturing(true)
            }}
            className={`shrink-0 rounded-md border px-2 py-1 font-mono text-[0.6875rem] ${
              hotkeyCapturing
                ? 'border-blue-400 bg-blue-500/10 text-blue-500'
                : 'border-line bg-surface-2 text-fg hover:bg-surface-3'
            }`}
          >
            {hotkeyCapturing ? t('settings.hotkeyPress') : formatBinding(settings.globalHotkey)}
          </button>
        </div>
        {hotkeyError && <div className="text-[0.65625rem] text-red-500">{hotkeyError}</div>}
        <ToggleRow
          label={t('settings.contextMenu')}
          hint={t('settings.contextMenuHint')}
          value={settings.contextMenu}
          onChange={(value) => onChange({ ...settings, contextMenu: value })}
        />
        <ToggleRow
          label={t('settings.associateFolders')}
          hint={t('settings.associateFoldersHint')}
          value={settings.associateFolders}
          onChange={(value) => onChange({ ...settings, associateFolders: value })}
        />
        <ToggleRow
          label={t('settings.autostart')}
          hint={t('settings.autostartHint')}
          value={settings.autostart}
          onChange={(value) => onChange({ ...settings, autostart: value })}
        />
        <ToggleRow
          label={t('settings.autostartMinimized')}
          hint={t('settings.autostartMinimizedHint')}
          value={settings.autostartMinimized}
          onChange={(value) => onChange({ ...settings, autostartMinimized: value })}
        />
        <div className="flex items-center justify-between gap-2 rounded-lg border border-line px-3 py-2">
          <div className="min-w-0">
            <div className="text-[0.71875rem] text-fg">{t('settings.integrationRemove')}</div>
            <div className="text-[0.625rem] text-muted-2">{t('settings.integrationRemoveHint')}</div>
          </div>
          <button
            type="button"
            onClick={() =>
              onChange({
                ...settings,
                hotkeyEnabled: false,
                contextMenu: false,
                associateFolders: false,
                autostart: false,
              })
            }
            className="shrink-0 rounded-md border border-line px-2 py-1 text-[0.6875rem] text-fg hover:bg-surface-3"
          >
            {t('settings.integrationRemove')}
          </button>
        </div>
      </div>
    </section>
  )
}
