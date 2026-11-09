import { useState } from 'react'
import type { Translate } from '../../i18n'
import type { ColorRule, Settings } from '../../persist'

type Props = {
  settings: Settings
  onChange: (next: Settings) => void
  t: Translate
  visible: boolean
}

const DEFAULT_FOREGROUND = '#2563eb'
const DEFAULT_BACKGROUND = '#fde047'

export function ColorRulesSection({ settings, onChange, t, visible }: Props) {
  const [editing, setEditing] = useState<number | null>(null)
  const [pattern, setPattern] = useState('')
  const [foregroundOn, setForegroundOn] = useState(true)
  const [foreground, setForeground] = useState(DEFAULT_FOREGROUND)
  const [backgroundOn, setBackgroundOn] = useState(false)
  const [background, setBackground] = useState(DEFAULT_BACKGROUND)
  const [folders, setFolders] = useState(false)

  const reset = () => {
    setEditing(null)
    setPattern('')
    setForegroundOn(true)
    setForeground(DEFAULT_FOREGROUND)
    setBackgroundOn(false)
    setBackground(DEFAULT_BACKGROUND)
    setFolders(false)
  }

  const startEdit = (index: number, rule: ColorRule) => {
    setEditing(index)
    setPattern(rule.pattern)
    setForegroundOn(!!rule.foreground)
    setForeground(rule.foreground || DEFAULT_FOREGROUND)
    setBackgroundOn(!!rule.background)
    setBackground(rule.background || DEFAULT_BACKGROUND)
    setFolders(rule.folders)
  }

  const commit = () => {
    const name = pattern.trim()
    if (!name) return
    const rule: ColorRule = {
      pattern: name,
      foreground: foregroundOn ? foreground : '',
      background: backgroundOn ? background : '',
      folders,
    }
    const rules = settings.colorRules
    const next =
      editing === null
        ? [...rules, rule]
        : rules.map((entry, index) => (index === editing ? rule : entry))
    onChange({ ...settings, colorRules: next })
    reset()
  }

  const move = (index: number, delta: number) => {
    const target = index + delta
    const rules = settings.colorRules
    if (target < 0 || target >= rules.length) return
    const next = rules.slice()
    ;[next[index], next[target]] = [next[target], next[index]]
    onChange({ ...settings, colorRules: next })
    // The editor tracks an array index, so it must follow the rule it was opened on.
    if (editing === index) setEditing(target)
    else if (editing === target) setEditing(index)
  }

  if (!visible) return null

  return (
    <section>
      <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">
        {t('settings.colorRules')}
      </h2>
      <div className="space-y-1.5 rounded-lg border border-line px-3 py-2" data-color-rules>
        <div className="pb-1 text-[0.65625rem] text-muted-2">{t('settings.colorRulesHint')}</div>
        {!settings.colorRules.length && (
          <div className="text-[0.6875rem] text-muted-2">{t('settings.colorEmpty')}</div>
        )}
        {settings.colorRules.map((rule, index) => (
          <div
            key={`${rule.pattern}-${index}`}
            data-color-rule={rule.pattern}
            data-color-editing={index === editing ? '1' : undefined}
            className={`flex items-center justify-between gap-2 rounded px-1 text-[0.71875rem] ${
              index === editing ? 'bg-surface-3' : ''
            }`}
          >
            <span className="flex min-w-0 items-center gap-2">
              <span
                className="h-3.5 w-3.5 shrink-0 rounded border border-line"
                style={{
                  backgroundColor: rule.background || 'transparent',
                  color: rule.foreground || undefined,
                }}
              />
              <span
                className="truncate font-mono text-fg"
                style={{
                  color: rule.foreground || undefined,
                  backgroundColor: rule.background || undefined,
                }}
              >
                {rule.pattern}
              </span>
              {rule.folders && (
                <span className="shrink-0 rounded border border-line px-1 text-[0.59375rem] text-muted-2">
                  {t('list.typeFolder')}
                </span>
              )}
            </span>
            <span className="flex shrink-0 items-center gap-0.5">
              <button
                type="button"
                onClick={() => move(index, -1)}
                disabled={index === 0}
                title={t('settings.colorMoveUp')}
                className="rounded px-1 py-0.5 text-[0.6875rem] text-muted hover:bg-surface-3 disabled:opacity-30"
              >
                ↑
              </button>
              <button
                type="button"
                onClick={() => move(index, 1)}
                disabled={index === settings.colorRules.length - 1}
                title={t('settings.colorMoveDown')}
                className="rounded px-1 py-0.5 text-[0.6875rem] text-muted hover:bg-surface-3 disabled:opacity-30"
              >
                ↓
              </button>
              <button
                type="button"
                onClick={() => startEdit(index, rule)}
                className="rounded px-1.5 py-0.5 text-[0.6875rem] text-muted hover:bg-surface-3 hover:text-accent"
              >
                {t('settings.placeEdit')}
              </button>
              <button
                type="button"
                onClick={() => {
                  if (editing === index) reset()
                  else if (editing !== null && index < editing) setEditing(editing - 1)
                  onChange({
                    ...settings,
                    colorRules: settings.colorRules.filter((_, i) => i !== index),
                  })
                }}
                className="rounded px-1.5 py-0.5 text-[0.6875rem] text-muted hover:bg-surface-3 hover:text-red-500"
              >
                {t('settings.placeDelete')}
              </button>
            </span>
          </div>
        ))}

        <div className="mt-1 space-y-1.5 border-t border-line pt-2">
          <input
            value={pattern}
            onChange={(ev) => setPattern(ev.target.value)}
            onKeyDown={(ev) => {
              if (ev.key === 'Enter') commit()
              if (ev.key === 'Escape' && editing !== null) reset()
            }}
            placeholder={t('settings.colorPatternPh')}
            title={t('settings.colorPattern')}
            data-color-pattern
            className="h-6 w-full rounded border border-line bg-surface-2 px-1.5 text-[0.6875rem] outline-none placeholder:text-muted-2 focus:border-accent-soft focus:bg-surface"
          />
          <div className="flex flex-wrap items-center gap-2 text-[0.65625rem] text-muted">
            <label className="flex items-center gap-1">
              <input type="checkbox" checked={foregroundOn} onChange={(ev) => setForegroundOn(ev.target.checked)} />
              {t('settings.colorForeground')}
              <input
                type="color"
                value={foreground}
                disabled={!foregroundOn}
                onChange={(ev) => setForeground(ev.target.value)}
                className="h-5 w-7 cursor-pointer rounded border border-line bg-transparent disabled:opacity-40"
              />
            </label>
            <label className="flex items-center gap-1">
              <input type="checkbox" checked={backgroundOn} onChange={(ev) => setBackgroundOn(ev.target.checked)} />
              {t('settings.colorBackground')}
              <input
                type="color"
                value={background}
                disabled={!backgroundOn}
                onChange={(ev) => setBackground(ev.target.value)}
                className="h-5 w-7 cursor-pointer rounded border border-line bg-transparent disabled:opacity-40"
              />
            </label>
            <label className="flex items-center gap-1">
              <input type="checkbox" checked={folders} onChange={(ev) => setFolders(ev.target.checked)} />
              {t('settings.colorFolders')}
            </label>
          </div>
          <div className="flex items-center gap-1.5">
            <button
              type="button"
              onClick={commit}
              disabled={!pattern.trim()}
              className="h-6 shrink-0 rounded bg-accent-strong px-2 text-[0.6875rem] text-white hover:bg-accent disabled:opacity-40"
            >
              {editing !== null ? t('settings.placeSave') : t('settings.placeAdd')}
            </button>
            {editing !== null && (
              <button
                type="button"
                onClick={reset}
                className="h-6 shrink-0 rounded border border-line px-2 text-[0.6875rem] text-fg hover:bg-surface-3"
              >
                {t('settings.placeCancel')}
              </button>
            )}
          </div>
        </div>
      </div>
    </section>
  )
}
