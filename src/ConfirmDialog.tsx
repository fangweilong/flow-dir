import { useEffect, useRef } from 'react'
import { makeT } from './i18n'
import type { Lang } from './types'
import { FileIcon } from './icons'

export type ConfirmItem = {
  name: string
  path?: string
}

type Props = {
  title: string
  message: string
  /** Optional item summary, kept small so the decision stays readable. */
  detail?: string
  /** Structured item list for actions operating on multiple files/items. */
  items?: (string | ConfirmItem)[]
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
  items,
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
        className="w-[28rem] max-w-[calc(100vw-2rem)] rounded-xl border border-line bg-surface p-4 shadow-2xl"
      >
        <div className="flex items-start gap-3">
          <div
            className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-lg ${
              danger
                ? 'bg-red-500/10 text-red-500 dark:bg-red-500/20'
                : 'bg-blue-500/10 text-blue-500 dark:bg-blue-500/20'
            }`}
          >
            {danger ? (
              <svg viewBox="0 0 16 16" className="h-5 w-5" fill="none" stroke="currentColor" strokeWidth="1.5">
                <path
                  d="M2.5 4h11M5.5 4V2.5a1 1 0 0 1 1-1h3a1 1 0 0 1 1 1V4M6.5 7v5M9.5 7v5M3.5 4l.8 9.2a1.5 1.5 0 0 0 1.5 1.3h4.4a1.5 1.5 0 0 0 1.5-1.3L12.5 4"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
              </svg>
            ) : (
              <svg viewBox="0 0 16 16" className="h-5 w-5" fill="none" stroke="currentColor" strokeWidth="1.5">
                <path
                  d="M2.5 4h11M5.5 4V2.5a1 1 0 0 1 1-1h3a1 1 0 0 1 1 1V4M3.5 4l.8 9.2a1.5 1.5 0 0 0 1.5 1.3h4.4a1.5 1.5 0 0 0 1.5-1.3L12.5 4"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
                <path d="M6.5 7.5 8 9l1.5-1.5M8 9V6.5" strokeLinecap="round" strokeLinejoin="round" />
              </svg>
            )}
          </div>
          <div className="min-w-0 flex-1 pt-0.5">
            <h2 className="text-[0.875rem] font-semibold text-fg-strong">{title}</h2>
            <p className="mt-0.5 text-[0.75rem] leading-relaxed text-muted">{message}</p>
          </div>
        </div>

        {items && items.length > 0 ? (
          <div className="mt-3 overflow-hidden rounded-lg border border-line bg-surface-2">
            {items.length > 1 && (
              <div className="flex items-center justify-between border-b border-line bg-surface-3/50 px-3 py-1.5 text-[0.6875rem] font-medium text-muted">
                <span>{t('list.items', { count: items.length })}</span>
              </div>
            )}
            <div className="no-bar max-h-44 overflow-y-auto divide-y divide-line/40">
              {items.map((item, idx) => {
                const name = typeof item === 'string' ? item : item.name
                const path = typeof item === 'string' ? undefined : item.path
                return (
                  <div
                    key={`${name}-${idx}`}
                    className="flex items-center gap-2 px-3 py-1.5 text-[0.75rem] transition-colors hover:bg-surface-3/40"
                  >
                    <FileIcon name={name} path={path} size={16} />
                    <span className="min-w-0 flex-1 select-all truncate text-fg" title={name}>
                      {name}
                    </span>
                  </div>
                )
              })}
            </div>
          </div>
        ) : detail ? (
          <div className="mt-2.5 max-h-24 overflow-auto break-all rounded-lg border border-line bg-surface-2 px-2.5 py-1.5 text-[0.6875rem] text-muted">
            {detail}
          </div>
        ) : null}

        <div className="mt-4 flex justify-end gap-2">
          <button
            ref={cancelRef}
            type="button"
            onClick={onClose}
            className="rounded-lg border border-line bg-surface px-3.5 py-1.5 text-[0.75rem] font-medium text-fg transition-colors hover:bg-surface-3 active:bg-surface-2"
          >
            {t('dialog.cancel')}
          </button>
          <button
            type="button"
            data-confirm-ok
            onClick={onConfirm}
            className={`rounded-lg px-3.5 py-1.5 text-[0.75rem] font-medium text-white shadow-sm transition-colors ${
              danger
                ? 'bg-red-600 hover:bg-red-500 active:bg-red-700'
                : 'bg-blue-600 hover:bg-blue-500 active:bg-blue-700'
            }`}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  )
}
