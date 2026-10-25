import { useEffect, useRef, useState } from 'react'
import { makeT } from './i18n'
import type { Lang } from './types'

type Props = {
  title: string
  initial: string
  lang: Lang
  onConfirm: (value: string) => void
  onClose: () => void
}

/** Small modal used for rename / new-folder name entry. */
export function PromptDialog({ title, initial, lang, onConfirm, onClose }: Props) {
  const t = makeT(lang)
  const [value, setValue] = useState(initial)
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    const input = inputRef.current
    if (!input) return
    input.focus()
    const dot = initial.lastIndexOf('.')
    input.setSelectionRange(0, dot > 0 ? dot : initial.length)
  }, [initial])

  const submit = () => {
    const next = value.trim()
    if (!next || next === initial) {
      onClose()
      return
    }
    onConfirm(next)
  }

  return (
    <div
      data-prompt
      className="fixed inset-0 z-[70] flex items-start justify-center bg-slate-950/30 pt-[18vh] backdrop-blur-[1px]"
      onMouseDown={(ev) => {
        if (ev.target === ev.currentTarget) onClose()
      }}
    >
      <div className="w-[380px] rounded-lg border border-line bg-surface p-3 shadow-2xl">
        <div className="pb-2 text-[12px] font-semibold text-fg-strong">{title}</div>
        <input
          data-prompt-input
          ref={inputRef}
          value={value}
          onChange={(ev) => setValue(ev.target.value)}
          onKeyDown={(ev) => {
            if (ev.key === 'Enter') {
              ev.preventDefault()
              submit()
            } else if (ev.key === 'Escape') {
              ev.preventDefault()
              onClose()
            }
          }}
          className="h-8 w-full rounded border border-line bg-surface-2 px-2 text-[12px] outline-none focus:border-blue-400"
        />
        <div className="flex justify-end gap-2 pt-3">
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-line px-3 py-1 text-[11.5px] text-fg hover:bg-surface-3"
          >
            {t('dialog.cancel')}
          </button>
          <button
            type="button"
            onClick={submit}
            className="rounded-md bg-blue-600 px-3 py-1 text-[11.5px] font-medium text-white hover:bg-blue-500"
          >
            {t('dialog.ok')}
          </button>
        </div>
      </div>
    </div>
  )
}
