import { useEffect, useRef, useState } from 'react'
import { COMPRESS_EXTENSIONS, COMPRESS_FORMATS, type CompressFormat } from './api'
import { makeT } from './i18n'
import type { Lang } from './types'

export type CompressValues = {
  format: CompressFormat
  level: number
  password: string | null
  name: string
}

type Props = {
  title: string
  dir: string
  name: string
  format: CompressFormat
  lang: Lang
  onSubmit: (values: CompressValues) => void
  onClose: () => void
}

/** Compression levels offered in the dialog; the backend takes 0-9. */
const LEVELS = [
  { level: 0, key: 'compress.levelStore' },
  { level: 3, key: 'compress.levelFast' },
  { level: 6, key: 'compress.levelNormal' },
  { level: 9, key: 'compress.levelMax' },
] as const

/** Swaps the archive extension when the format changes, keeping the rest of the name. */
function rename(name: string, from: CompressFormat, to: CompressFormat) {
  const old = COMPRESS_EXTENSIONS[from]
  const next = COMPRESS_EXTENSIONS[to]
  return name.toLowerCase().endsWith(old) ? name.slice(0, -old.length) + next : name + next
}

/** Modal that picks the archive format, level, password and file name. */
export function CompressDialog({ title, dir, name, format, lang, onSubmit, onClose }: Props) {
  const t = makeT(lang)
  const [pick, setPick] = useState<CompressFormat>(format)
  const [level, setLevel] = useState(6)
  const [password, setPassword] = useState('')
  const [value, setValue] = useState(name)
  const nameRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    const input = nameRef.current
    if (!input) return
    input.focus()
    // Archive names can carry several dots (`a.tar.gz`): select up to the first one.
    const dot = name.indexOf('.')
    input.setSelectionRange(0, dot > 0 ? dot : name.length)
  }, [name])

  const encrypted = pick === 'zip'
  const submit = () => {
    const trimmed = value.trim()
    if (!trimmed) return
    onSubmit({ format: pick, level, password: encrypted && password ? password : null, name: trimmed })
  }

  const label = 'text-[0.6875rem] text-muted'
  const control =
    'h-8 w-full rounded border border-line bg-surface-2 px-2 text-[0.75rem] text-fg outline-none focus:border-blue-400'

  return (
    <div
      data-compress
      className="fixed inset-0 z-[70] flex items-center justify-center bg-slate-950/30 p-4 backdrop-blur-[1px]"
      onMouseDown={(ev) => {
        if (ev.target === ev.currentTarget) onClose()
      }}
    >
      <div className="w-[25rem] rounded-lg border border-line bg-surface p-3 shadow-2xl">
        <div className="pb-2.5 text-[0.75rem] font-semibold text-fg-strong">{title}</div>
        <div className="grid grid-cols-[4.5rem_1fr] items-center gap-x-2 gap-y-2">
          <span className={label}>{t('compress.format')}</span>
          <select
            data-compress-format
            value={pick}
            onChange={(ev) => {
              const next = ev.target.value as CompressFormat
              setValue((current) => rename(current, pick, next))
              setPick(next)
            }}
            className={control}
          >
            {COMPRESS_FORMATS.map((item) => (
              <option key={item} value={item}>
                {item.toUpperCase()}
              </option>
            ))}
          </select>

          <span className={label}>{t('compress.level')}</span>
          <select
            data-compress-level
            value={level}
            onChange={(ev) => setLevel(Number(ev.target.value))}
            className={control}
          >
            {LEVELS.map((item) => (
              <option key={item.level} value={item.level}>
                {t(item.key)}
              </option>
            ))}
          </select>

          <span className={label}>{t('compress.password')}</span>
          <input
            data-compress-password
            type="password"
            value={password}
            disabled={!encrypted}
            placeholder={encrypted ? '' : t('compress.passwordZipOnly')}
            onChange={(ev) => setPassword(ev.target.value)}
            className={`${control} disabled:cursor-not-allowed disabled:opacity-50`}
          />

          <span className={label}>{t('compress.name')}</span>
          <input
            data-compress-name
            ref={nameRef}
            value={value}
            onChange={(ev) => setValue(ev.target.value)}
            onKeyDown={(ev) => {
              // Enter/Escape inside an IME composition belong to the candidate window.
              if (ev.nativeEvent.isComposing) return
              if (ev.key === 'Enter') {
                ev.preventDefault()
                submit()
              } else if (ev.key === 'Escape') {
                ev.preventDefault()
                onClose()
              }
            }}
            className={control}
          />
        </div>
        <div className="truncate pt-2 text-[0.65625rem] text-muted" title={dir}>
          {dir}
        </div>
        <div className="flex justify-end gap-2 pt-3">
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-line px-3 py-1 text-[0.71875rem] text-fg hover:bg-surface-3"
          >
            {t('dialog.cancel')}
          </button>
          <button
            type="button"
            onClick={submit}
            disabled={!value.trim()}
            className="rounded-md bg-blue-600 px-3 py-1 text-[0.71875rem] font-medium text-white hover:bg-blue-500 disabled:opacity-50"
          >
            {t('compress.apply')}
          </button>
        </div>
      </div>
    </div>
  )
}
