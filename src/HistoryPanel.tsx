import { makeT } from './i18n'
import type { Lang } from './types'
import { baseName, parentPath } from './paths'
import type { UndoEntry } from './undo'

type Props = {
  lang: Lang
  undo: UndoEntry[]
  redo: UndoEntry[]
  onUndoTo: (id: number) => void
  onRedo: () => void
  onClose: () => void
}

const KIND_KEY = {
  rename: 'history.rename',
  move: 'history.move',
  copy: 'history.copy',
  delete: 'history.delete',
} as const

/** One-line summary: the operation, its item count and the folder it touched. */
function describe(entry: UndoEntry, t: ReturnType<typeof makeT>): string {
  const count = entry.request.sources.length
  const first = entry.request.sources[0] ?? ''
  const folder = first ? parentPath(first) : ''
  const name = first ? baseName(first) : ''
  const label = t(KIND_KEY[entry.kind])
  return count > 1
    ? t('history.summaryMany', { label, count, folder })
    : t('history.summary', { label, name, folder })
}

/**
 * Undo/redo list. Rows are newest first; clicking one replays every step after it, which is
 * the same thing as pressing Ctrl+Z that many times.
 */
export function HistoryPanel({ lang, undo, redo, onUndoTo, onRedo, onClose }: Props) {
  const t = makeT(lang)
  const format = (at: number) =>
    new Date(at).toLocaleTimeString(lang === 'zh-CN' ? 'zh-CN' : 'en-US', {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    })
  return (
    <div
      data-history-panel
      role="dialog"
      aria-label={t('history.title')}
      className="fixed bottom-10 right-4 z-50 flex max-h-[60vh] w-[22rem] flex-col rounded-lg border border-line bg-surface/95 shadow-lg backdrop-blur"
    >
      <div className="flex items-center justify-between border-b border-line px-2.5 py-1.5">
        <span className="text-[0.71875rem] font-medium text-fg-strong">{t('history.title')}</span>
        <button
          type="button"
          onClick={onClose}
          className="rounded border border-line px-1.5 py-0.5 text-[0.65625rem] text-muted hover:bg-surface-3 hover:text-fg"
        >
          {t('op.close')}
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-1.5">
        {undo.length === 0 && redo.length === 0 ? (
          <div className="px-1 py-2 text-[0.6875rem] text-muted-2">{t('undo.empty')}</div>
        ) : null}
        <ul className="space-y-0.5">
          {[...undo].reverse().map((entry, index) => (
            <li key={entry.id}>
              <button
                type="button"
                data-history-undo
                onClick={() => onUndoTo(entry.id)}
                className={`w-full rounded px-1.5 py-1 text-left hover:bg-surface-3 ${
                  index === 0 ? 'bg-surface-2' : ''
                }`}
              >
                <div className="flex items-baseline justify-between gap-2">
                  <span className="truncate text-[0.6875rem] text-fg">{describe(entry, t)}</span>
                  <span className="shrink-0 text-[0.625rem] tabular text-muted-2">{format(entry.at)}</span>
                </div>
                <div className="truncate text-[0.625rem] text-muted-2">
                  {index === 0 ? t('history.nextUndo') : t('history.undoThrough')}
                </div>
              </button>
            </li>
          ))}
        </ul>
        {redo.length > 0 && (
          <div className="mt-1.5 border-t border-line pt-1.5">
            <div className="px-1.5 pb-1 text-[0.625rem] font-semibold tracking-wide text-muted-2">
              {t('history.redoSection')}
            </div>
            <button
              type="button"
              data-history-redo
              onClick={onRedo}
              className="w-full rounded px-1.5 py-1 text-left hover:bg-surface-3"
            >
              <div className="flex items-baseline justify-between gap-2">
                <span className="truncate text-[0.6875rem] text-fg">
                  {describe(redo[redo.length - 1], t)}
                </span>
                <span className="shrink-0 text-[0.625rem] tabular text-muted-2">
                  {format(redo[redo.length - 1].at)}
                </span>
              </div>
              <div className="text-[0.625rem] text-muted-2">
                {t('history.redoCount', { count: redo.length })}
              </div>
            </button>
          </div>
        )}
      </div>
    </div>
  )
}
