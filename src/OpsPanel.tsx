import { useEffect, useRef, useState } from 'react'
import { errorKey, makeT } from './i18n'
import type { Translate } from './i18n'
import { formatSize } from './paths'
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
  start: 'op.working',
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

/** Seconds for the expanded "remaining" row; a card never spans days. */
function formatDuration(seconds: number) {
  const total = Math.max(0, Math.round(seconds))
  if (total < 60) return `${total}s`
  const minutes = Math.floor(total / 60)
  if (minutes < 60) return `${minutes}m ${total % 60}s`
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`
}

/**
 * Locally sampled transfer rate: the backend only reports cumulative bytes, and progress
 * messages arrive every ~120 ms, so sampling faster than half a second would only amplify
 * their jitter.
 */
function useTransferRate(op: OpProgress) {
  const [rate, setRate] = useState<{ bps: number; eta: number | null } | null>(null)
  const sample = useRef<{ at: number; work: number } | null>(null)
  useEffect(() => {
    if (op.done) {
      // A finished card must not keep showing the speed it had before the last item.
      setRate(null)
      return
    }
    const now = performance.now()
    const previous = sample.current
    if (!previous) {
      sample.current = { at: now, work: op.work }
      return
    }
    const seconds = (now - previous.at) / 1000
    if (seconds < 0.5) return
    const delta = op.work - previous.work
    sample.current = { at: now, work: op.work }
    if (delta <= 0) return
    const bps = delta / seconds
    setRate({
      bps,
      eta: op.workTotal > op.work ? (op.workTotal - op.work) / bps : null,
    })
  }, [op.done, op.work, op.workTotal])
  return rate
}

function DetailRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline gap-2">
      <dt className="w-14 shrink-0 whitespace-nowrap text-muted-2">{label}</dt>
      <dd className="min-w-0 flex-1 truncate text-muted" title={value}>
        {value}
      </dd>
    </div>
  )
}

type CardProps = Omit<Props, 'lang' | 'ops'> & { op: OpProgress; t: Translate }

/** One operation card; expanded details are local to the card so updates keep them open. */
function OpCard({ op, t, onCancel, onDismiss, onRetryElevated, elevating }: CardProps) {
  const [expanded, setExpanded] = useState(false)
  const rate = useTransferRate(op)
  const key = PHASE_KEY[op.phase as keyof typeof PHASE_KEY]
  const phaseText = key ? t(key) : t('op.working')
  // A finished card must not keep saying "working"; failures stay readable there.
  const label = op.done
    ? op.failedCount > 0
      ? t('op.partial', {
          ok: Math.max(0, op.total - op.failedCount),
          failed: op.failedCount,
        })
      : t('op.done')
    : phaseText
  const percent =
    op.workTotal > 0
      ? Math.min(100, Math.round((op.work / op.workTotal) * 100))
      : op.total > 0
        ? Math.min(100, Math.round((op.current / op.total) * 100))
        : 0
  // WebDAV reports transferred bytes without a total, so the data row falls back to the
  // amount done so far instead of hiding an otherwise useful figure.
  const workText =
    op.workTotal > 0
      ? `${formatSize(op.work, false)} / ${formatSize(op.workTotal, false)}`
      : op.work > 0
        ? formatSize(op.work, false)
        : null
  const detailsId = `op-details-${op.opId}`
  return (
    <div
      data-op={op.opId}
      className="rounded-lg border border-line bg-surface/95 p-2.5 shadow-lg backdrop-blur"
    >
      <div className="flex items-center justify-between gap-2">
        <button
          type="button"
          data-op-toggle
          aria-expanded={expanded}
          aria-controls={detailsId}
          title={expanded ? t('op.collapse') : t('op.expand')}
          onClick={() => setExpanded((value) => !value)}
          className="flex min-w-0 flex-1 items-center gap-1 rounded text-left"
        >
          <svg
            viewBox="0 0 16 16"
            aria-hidden
            className={`h-2.5 w-2.5 shrink-0 text-muted-2 transition-transform ${expanded ? 'rotate-90' : ''}`}
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
          >
            <path d="M6 4l4 4-4 4" />
          </svg>
          <span className="truncate text-[0.71875rem] font-medium text-fg-strong">
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
        </button>
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
      {!expanded && op.name && (
        <div className="mt-1 truncate text-[0.65625rem] text-muted" title={op.name}>
          {op.name}
        </div>
      )}
      <div className="mt-1.5 h-1 overflow-hidden rounded-full bg-surface-3">
        <div className="h-full rounded-full bg-accent transition-[width]" style={{ width: `${percent}%` }} />
      </div>
      <div className="mt-1 text-right text-[0.625rem] tabular text-muted-2">{percent}%</div>
      {expanded && (
        <dl
          id={detailsId}
          data-op-details
          className="mt-1.5 space-y-0.5 border-t border-line pt-1.5 text-[0.625rem]"
        >
          {!op.done && <DetailRow label={t('op.detailPhase')} value={phaseText} />}
          {op.total > 0 && (
            <DetailRow
              label={t('op.detailItems')}
              value={`${Math.min(op.current, op.total)} / ${op.total}`}
            />
          )}
          {workText && <DetailRow label={t('op.detailData')} value={workText} />}
          {!op.done && rate && (
            <DetailRow label={t('op.detailSpeed')} value={`${formatSize(rate.bps, false)}/s`} />
          )}
          {!op.done && rate?.eta != null && (
            <DetailRow label={t('op.detailRemaining')} value={formatDuration(rate.eta)} />
          )}
          {op.name && <DetailRow label={t('op.detailCurrent')} value={op.name} />}
          {op.error && (
            <DetailRow
              label={t('op.detailError')}
              value={
                op.error.detail
                  ? `${t(errorKey(op.error.code))}: ${op.error.detail}`
                  : t(errorKey(op.error.code))
              }
            />
          )}
        </dl>
      )}
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
            className="mt-1.5 w-full rounded border border-accent/40 bg-accent/10 px-1.5 py-1 text-[0.65625rem] text-accent-strong hover:bg-accent/20 disabled:opacity-50 dark:text-accent-faint"
          >
            {t('op.retryElevated')}
          </button>
        )}
    </div>
  )
}

/** Bottom-right stack of running operations. */
export function OpsPanel({ ops, lang, onCancel, onDismiss, onRetryElevated, elevating }: Props) {
  const t = makeT(lang)
  if (!ops.length) return null
  return (
    <div className="fixed bottom-10 right-4 z-50 flex w-[18.75rem] flex-col gap-2">
      {ops.map((op) => (
        <OpCard
          key={op.opId}
          op={op}
          t={t}
          onCancel={onCancel}
          onDismiss={onDismiss}
          onRetryElevated={onRetryElevated}
          elevating={elevating}
        />
      ))}
    </div>
  )
}
