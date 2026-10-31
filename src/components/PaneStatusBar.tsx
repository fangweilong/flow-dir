import { formatSize } from '../paths'
import type { Translate } from '../i18n'
import type { GitStatus } from '../types'
import { GitChip } from './GitBadges'

export interface PaneStatusBarProps {
  git: GitStatus | null
  t: Translate
  /** True when the pane shows the drive overview instead of a folder listing. */
  root: boolean
  driveCount: number
  visibleCount: number
  entryCount: number
  filter: string
  selectedCount: number
  selectedSize: number
  loading: boolean
  scanned: number
  path: string | undefined
}

/** Footer row: git chip, item/selection counts and the active tab's path. */
export function PaneStatusBar({
  git,
  t,
  root,
  driveCount,
  visibleCount,
  entryCount,
  filter,
  selectedCount,
  selectedSize,
  loading,
  scanned,
  path,
}: PaneStatusBarProps) {
  return (
    <div
      data-status
      className="flex h-6 shrink-0 items-center justify-between border-t border-line bg-surface-2 px-2 text-[0.65625rem] text-muted"
    >
      <div className="flex min-w-0 items-center gap-2">
        {git?.root && <GitChip git={git} t={t} />}
        <span>{root ? t('list.drives', { count: driveCount }) : t('list.items', { count: visibleCount })}</span>
        {filter && entryCount !== visibleCount && <span>{t('list.total', { count: entryCount })}</span>}
        {selectedCount > 0 && (
          <span>
            {t('list.selected', { count: selectedCount })}
            {selectedSize > 0 && ` · ${formatSize(selectedSize, false)}`}
          </span>
        )}
        {loading && entryCount > 0 && <span>{t('list.scanning', { count: scanned })}</span>}
      </div>
      <span className="max-w-[45%] truncate font-mono text-[0.59375rem] text-muted-2">{path}</span>
    </div>
  )
}