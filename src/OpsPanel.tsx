import { makeT } from './i18n'
import type { Lang, OpProgress } from './types'

type Props = {
  ops: OpProgress[]
  lang: Lang
  onCancel: (opId: string) => void
}

const PHASE_KEY = {
  copy: 'op.copying',
  move: 'op.moving',
  delete: 'op.deleting',
  rename: 'op.renaming',
  newdir: 'op.creating',
} as const

/** Bottom-right stack of running operations. */
export function OpsPanel({ ops, lang, onCancel }: Props) {
  const t = makeT(lang)
  if (!ops.length) return null
  return (
    <div className="fixed bottom-10 right-4 z-50 flex w-[300px] flex-col gap-2">
      {ops.map((op) => {
        const key = PHASE_KEY[op.phase as keyof typeof PHASE_KEY]
        const label = key ? t(key) : t('op.working')
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
              <span className="text-[11.5px] font-medium text-fg-strong">
                {label}
                {op.total > 1 && (
                  <span className="ml-1.5 font-normal text-muted-2">
                    {t('op.items', { current: Math.min(op.current, op.total), total: op.total })}
                  </span>
                )}
              </span>
              <button
                type="button"
                data-op-cancel
                onClick={() => onCancel(op.opId)}
                className="rounded border border-line px-1.5 py-0.5 text-[10.5px] text-muted hover:bg-surface-3 hover:text-fg"
              >
                {t('op.cancel')}
              </button>
            </div>
            {op.name && (
              <div className="mt-1 truncate text-[10.5px] text-muted" title={op.name}>
                {op.name}
              </div>
            )}
            <div className="mt-1.5 h-1 overflow-hidden rounded-full bg-surface-3">
              <div className="h-full rounded-full bg-blue-500 transition-[width]" style={{ width: `${percent}%` }} />
            </div>
            <div className="mt-1 text-right text-[10px] tabular text-muted-2">{percent}%</div>
          </div>
        )
      })}
    </div>
  )
}
