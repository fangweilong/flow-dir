import { useEffect, useRef } from 'react'
import { makeT } from './i18n'
import type { Lang } from './types'

type Props = {
  title: string
  message: string
  /** Optional item summary, kept small so the decision stays readable. */
  detail?: string
  confirmLabel: string
  danger?: boolean
  lang: Lang
  onConfirm: () => void
  onClose: () => void
}

/**
 * Blocking confirmation for destructive actions. Focus starts on Cancel so a stray Enter
 * never deletes anything; Enter still confirms once the user has deliberately moved focus.
 */
export function ConfirmDialog({
  title,
  message,
  detail,
  confirmLabel,
  danger,
  lang,
  onConfirm,
  onClose,
}: Props) {
  const t = makeT(lang)
  const cancelRef = useRef<HTMLButtonElement>(null)

  useEffect(() => {
    cancelRef.current?.focus()
  }, [])

  useEffect(() => {
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === 'Escape') {
        ev.preventDefault()
        ev.stopPropagation()
        onClose()
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [onClose])

  return (
    <div
      data-confirm
      className="fixed inset-0 z-[75] flex items-center justify-center bg-slate-950/30 p-4 backdrop-blur-[1px]"
      onMouseDown={(ev) => {
        if (ev.target === ev.currentTarget) onClose()
      }}
    >
      <div
        role="alertdialog"
        aria-modal="true"
        className="w-[26rem] rounded-lg border border-line bg-surface p-3 shadow-2xl"
      >
        <div className="pb-1.5 text-[0.75rem] font-semibold text-fg-strong">{title}</div>
        <div className="text-[0.71875rem] leading-relaxed text-fg">{message}</div>
        {detail && (
          <div className="mt-1.5 max-h-24 overflow-auto break-all rounded border border-line bg-surface-2 px-2 py-1 text-[0.6875rem] text-muted">
            {detail}
          </div>
        )}
        <div className="flex justify-end gap-2 pt-3">
          <button
            ref={cancelRef}
            type="button"
            onClick={onClose}
            className="rounded-md border border-line px-3 py-1 text-[0.71875rem] text-fg hover:bg-surface-3"
          >
            {t('dialog.cancel')}
          </button>
          <button
            type="button"
            data-confirm-ok
            onClick={onConfirm}
            className={`rounded-md px-3 py-1 text-[0.71875rem] font-medium text-white ${
              danger ? 'bg-red-600 hover:bg-red-500' : 'bg-blue-600 hover:bg-blue-500'
            }`}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  )
}
