import { errorKey, makeT } from './i18n'
import type { Lang, OpProgress } from './types'

type Props = {
  ops: OpProgress[]
  lang: Lang
  onCancel: (opId: string) => void
  /** Removes a finished card, including one kept around to show its failure list. */
  onDismiss: (opId: string) => void
  /** Re-runs a denied operation through the elevated helper. */
  onRetryElevated: (opId: string) => void
  /** Op id whose UAC prompt is open; its retry button stays disabled until it returns. */
  elevating?: string | null
}

const PHASE_KEY = {
  plan: 'op.planning',
  extract: 'op.extracting',
  compress: 'op.compressing',
  merge: 'op.merging',
  copy: 'op.copying',
  move: 'op.moving',
  delete: 'op.deleting',
  rename: 'op.renaming',
  newdir: 'op.creating',
} as const

/** Bottom-right stack of running operations. */
export function OpsPanel({ ops, lang, onCancel, onDismiss, onRetryElevated, elevating }: Props) {
  const t = makeT(lang)
  if (!ops.length) return null
  return (
    <div className="fixed bottom-10 right-4 z-50 flex w-[18.75rem] flex-col gap-2">
      {ops.map((op) => {
        const key = PHASE_KEY[op.phase as keyof typeof PHASE_KEY]
        // A finished card must not keep saying "working"; failures stay readable there.
        const label = op.done
          ? op.failedCount > 0
            ? t('op.partial', {
                ok: Math.max(0, op.total - op.failedCount),
                failed: op.failedCount,
              })
            : t('op.done')
          : key
            ? t(key)
            : t('op.working')
        const percent =
          op.workTotal > 0
            ? Math.min(100, Math.round((op.work / op.workTotal) * 100))
            : op.total > 0
              ? Math.min(100, Math.round((op.current / op.total) * 100))
              : 0
        return (
          <div
            key={op.opId}
            data-op={op.opId}
            className="rounded-lg border border-line bg-surface/95 p-2.5 shadow-lg backdrop-blur"
          >
            <div className="flex items-center justify-between gap-2">
              <span className="text-[0.71875rem] font-medium text-fg-strong">
                {label}
                {op.total > 1 && (
                  <span className="ml-1.5 font-normal text-muted-2">
                    {t('op.items', { current: Math.min(op.current, op.total), total: op.total })}
                  </span>
                )}
                {!op.done && op.failedCount > 0 && (
                  <span className="ml-1.5 font-normal text-red-500">
                    {t('op.failedCount', { count: op.failedCount })}
                  </span>
                )}
              </span>
              {op.done ? (
                <button
                  type="button"
                  data-op-dismiss
                  onClick={() => onDismiss(op.opId)}
                  className="rounded border border-line px-1.5 py-0.5 text-[0.65625rem] text-muted hover:bg-surface-3 hover:text-fg"
                >
                  {t('op.close')}
                </button>
              ) : (
                <button
                  type="button"
                  data-op-cancel
                  onClick={() => onCancel(op.opId)}
                  className="rounded border border-line px-1.5 py-0.5 text-[0.65625rem] text-muted hover:bg-surface-3 hover:text-fg"
                >
                  {t('op.cancel')}
                </button>
              )}
            </div>
            {op.name && (
              <div className="mt-1 truncate text-[0.65625rem] text-muted" title={op.name}>
                {op.name}
              </div>
            )}
            <div className="mt-1.5 h-1 overflow-hidden rounded-full bg-surface-3">
              <div className="h-full rounded-full bg-blue-500 transition-[width]" style={{ width: `${percent}%` }} />
            </div>
            <div className="mt-1 text-right text-[0.625rem] tabular text-muted-2">{percent}%</div>
            {op.failed.length > 0 && (
              <ul
                data-op-failures
                className="mt-1 max-h-32 overflow-auto border-t border-line pt-1 text-[0.625rem]"
              >
                {op.failed.map((failure) => (
                  <li
                    key={`${failure.path}:${failure.code}`}
                    title={failure.detail}
                    className="flex items-baseline gap-1 py-0.5"
                  >
                    <span className="shrink-0 text-red-500">{t(errorKey(failure.code))}</span>
                    <span className="truncate text-muted">{failure.path || failure.detail}</span>
                  </li>
                ))}
              </ul>
            )}
            {op.done &&
              (op.error?.code === 'ACCESS_DENIED' ||
                op.failed.some((failure) => failure.code === 'ACCESS_DENIED')) && (
                <button
                  type="button"
                  data-op-elevate
                  disabled={elevating === op.opId}
                  onClick={() => onRetryElevated(op.opId)}
                  className="mt-1.5 w-full rounded border border-blue-500/40 bg-blue-500/10 px-1.5 py-1 text-[0.65625rem] text-blue-600 hover:bg-blue-500/20 disabled:opacity-50 dark:text-blue-300"
                >
                  {t('op.retryElevated')}
                </button>
              )}
          </div>
        )
      })}
    </div>
  )
}
