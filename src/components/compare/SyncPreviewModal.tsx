import type { Translate } from '../../i18n'
import type { SyncAction, SyncPlan } from '../../ComparePanel'

export interface SyncPreviewModalProps {
  plan: SyncPlan
  strategy: 'l2r' | 'r2l' | 'both'
  setStrategy: (strategy: 'l2r' | 'r2l' | 'both') => void
  deleteExtras: boolean
  setDeleteExtras: (val: boolean) => void
  onClose: () => void
  onExecute: () => void
  executing: boolean
  t: Translate
}

export function SyncPreviewModal({
  plan,
  strategy,
  setStrategy,
  deleteExtras,
  setDeleteExtras,
  onClose,
  onExecute,
  executing,
  t,
}: SyncPreviewModalProps) {
  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-slate-950/40 p-4">
      <div className="flex max-h-[80vh] w-[46rem] flex-col overflow-hidden rounded-xl border border-line bg-surface shadow-2xl">
        <div className="flex items-center justify-between border-b border-line px-3 py-2">
          <span className="text-[0.8125rem] font-semibold text-fg-strong">{t('compare.previewTitle')}</span>
          <button
            type="button"
            onClick={onClose}
            aria-label={t('compare.close')}
            className="grid h-6 w-6 place-items-center rounded text-muted hover:bg-surface-3 hover:text-fg"
          >
            ✕
          </button>
        </div>
        <div className="flex flex-wrap items-center gap-2 border-b border-line px-3 py-1.5 text-[0.65625rem] text-muted-2">
          <label className="flex items-center gap-1">
            {t('compare.strategy')}
            <select
              value={strategy}
              onChange={(e) => {
                setStrategy(e.target.value as 'l2r' | 'r2l' | 'both')
              }}
              className="h-6 rounded border border-line bg-surface-2 px-1 text-fg outline-none"
            >
              <option value="l2r">{t('compare.strategyL2R')}</option>
              <option value="r2l">{t('compare.strategyR2L')}</option>
              <option value="both">{t('compare.strategyBoth')}</option>
            </select>
          </label>
          <label className="flex items-center gap-1">
            <input
              type="checkbox"
              checked={deleteExtras}
              onChange={(e) => {
                setDeleteExtras(e.target.checked)
              }}
            />
            {t('compare.deleteExtra')}
          </label>
          <span className="ml-auto">
            {t('compare.actionCount', { count: plan.actions.length })}
            {plan.conflicts ? ` · ${t('compare.conflicts', { count: plan.conflicts })}` : ''}
            {plan.mismatches ? ` · ${t('compare.mismatches', { count: plan.mismatches })}` : ''}
          </span>
        </div>
        <div className="min-h-0 flex-1 overflow-auto">
          {plan.actions.length === 0 ? (
            <div className="px-3 py-8 text-center text-[0.71875rem] text-muted-2">{t('compare.previewEmpty')}</div>
          ) : (
            <ul className="divide-y divide-row-line text-[0.6875rem]">
              {plan.actions.map((action: SyncAction, index: number) => (
                <li key={index} className="flex items-center gap-2 px-3 py-1">
                  <span
                    className={`w-16 shrink-0 rounded px-1 text-center text-[0.625rem] ${
                      action.kind === 'delete'
                        ? 'bg-rose-100 text-rose-700 dark:bg-rose-950 dark:text-rose-300'
                        : 'bg-blue-100 text-blue-700 dark:bg-blue-950 dark:text-blue-300'
                    }`}
                  >
                    {action.kind === 'delete' ? t('compare.action.delete') : t('compare.action.copy')}
                  </span>
                  <span className="min-w-0 flex-1 truncate text-fg" title={action.kind === 'copy' ? action.source : action.paths[0]}>
                    {action.kind === 'copy' ? action.source : action.paths.join(', ')}
                  </span>
                  <span className="shrink-0 text-muted-2">
                    → {action.kind === 'copy' ? action.targetDir : t('compare.recycle')}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
        <div className="flex items-center justify-between gap-2 border-t border-line px-3 py-2 text-[0.65625rem] text-muted-2">
          <span>{t('compare.loadedOnly')}</span>
          <div className="flex items-center gap-1">
            <button
              type="button"
              onClick={onClose}
              className="rounded border border-line px-2 py-1 hover:text-fg"
            >
              {t('compare.close')}
            </button>
            <button
              type="button"
              onClick={onExecute}
              disabled={executing || !plan.actions.length}
              className="rounded bg-blue-600 px-2.5 py-1 text-white hover:bg-blue-500 disabled:opacity-40"
            >
              {executing ? t('compare.executing') : t('compare.execute')}
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
