import { useState } from 'react'
import type { Translate } from '../../i18n'
import type { Settings } from '../../persist'
import { pickFolder } from '../../api'

export type ProtectionSectionProps = {
  settings: Settings
  onChange: (next: Settings) => void
  t: Translate
  visible: boolean
}

const samePath = (a: string, b: string) => a.toLowerCase() === b.toLowerCase()

/** Appends `path` unless it is already present; the caller removes it from the other list. */
const appendPath = (list: string[], path: string) =>
  list.some((entry) => samePath(entry, path)) ? list : [...list, path]

type PathListProps = {
  label: string
  hint: string
  paths: string[]
  tone: 'block' | 'confirm'
  t: Translate
  onAdd: (path: string) => void
  onRemove: (path: string) => void
}

function PathList({ label, hint, paths, tone, t, onAdd, onRemove }: PathListProps) {
  const [draft, setDraft] = useState('')
  const add = () => {
    const path = draft.trim()
    if (!path) return
    onAdd(path)
    setDraft('')
  }

  return (
    <div className="space-y-1.5 border-t border-line pt-2" data-protection={tone}>
      <div>
        <div
          className={`text-[0.71875rem] ${tone === 'block' ? 'text-red-500' : 'text-accent'}`}
        >
          {label}
        </div>
        <div className="text-[0.625rem] text-muted-2">{hint}</div>
      </div>
      {paths.length ? (
        paths.map((path) => (
          <div key={path} className="flex items-center justify-between gap-2 text-[0.71875rem]">
            <span className="min-w-0 truncate font-mono text-[0.65625rem] text-fg" title={path}>
              {path}
            </span>
            <button
              type="button"
              onClick={() => onRemove(path)}
              className="shrink-0 rounded px-1.5 py-0.5 text-[0.6875rem] text-muted hover:bg-surface-3 hover:text-red-500"
            >
              {t('settings.placeDelete')}
            </button>
          </div>
        ))
      ) : (
        <div className="text-[0.65625rem] text-muted-2">{t('settings.protectionEmpty')}</div>
      )}
      <div className="flex items-center gap-1.5">
        <input
          value={draft}
          onChange={(ev) => setDraft(ev.target.value)}
          onKeyDown={(ev) => {
            if (ev.key === 'Enter') add()
          }}
          placeholder={t('settings.protectionPathPh')}
          className="h-6 min-w-0 flex-1 rounded border border-line bg-surface-2 px-1.5 text-[0.6875rem] outline-none placeholder:text-muted-2 focus:border-accent-soft focus:bg-surface"
        />
        <button
          type="button"
          onClick={() => {
            void pickFolder(t('settings.protectionPathPh'), draft)
              .then((picked) => {
                if (picked) setDraft(picked)
              })
              .catch(() => undefined)
          }}
          className="h-6 shrink-0 rounded border border-line px-1.5 text-[0.6875rem] text-fg hover:bg-surface-3"
        >
          {t('settings.placeBrowse')}
        </button>
        <button
          type="button"
          onClick={add}
          disabled={!draft.trim()}
          className="h-6 shrink-0 rounded bg-accent-strong px-2 text-[0.6875rem] text-white hover:bg-accent disabled:opacity-40"
        >
          {t('settings.placeAdd')}
        </button>
      </div>
    </div>
  )
}

export function ProtectionSection({ settings, onChange, t, visible }: ProtectionSectionProps) {
  if (!visible) return null

  const addBlock = (path: string) =>
    onChange({
      ...settings,
      protectBlock: appendPath(settings.protectBlock, path),
      protectConfirm: settings.protectConfirm.filter((entry) => !samePath(entry, path)),
    })
  const addConfirm = (path: string) =>
    onChange({
      ...settings,
      protectConfirm: appendPath(settings.protectConfirm, path),
      protectBlock: settings.protectBlock.filter((entry) => !samePath(entry, path)),
    })

  return (
    <section>
      <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">
        {t('settings.protection')}
      </h2>
      <div className="space-y-3 rounded-lg border border-line px-3 py-2">
        <div className="text-[0.65625rem] text-muted-2">{t('settings.protectionHint')}</div>
        <PathList
          label={t('settings.protectionBlock')}
          hint={t('settings.protectionBlockHint')}
          paths={settings.protectBlock}
          tone="block"
          t={t}
          onAdd={addBlock}
          onRemove={(path) =>
            onChange({
              ...settings,
              protectBlock: settings.protectBlock.filter((entry) => entry !== path),
            })
          }
        />
        <PathList
          label={t('settings.protectionConfirm')}
          hint={t('settings.protectionConfirmHint')}
          paths={settings.protectConfirm}
          tone="confirm"
          t={t}
          onAdd={addConfirm}
          onRemove={(path) =>
            onChange({
              ...settings,
              protectConfirm: settings.protectConfirm.filter((entry) => entry !== path),
            })
          }
        />
        <div className="border-t border-line pt-2 text-[0.625rem] text-muted-2">
          {t('settings.protectionAlways')}
        </div>
      </div>
    </section>
  )
}
