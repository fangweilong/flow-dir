import { formatSize } from '../paths'
import type { Translate } from '../i18n'
import type { GitStatus } from '../types'
import type { ViewMode } from '../workspace'
import type { FolderSizeState } from '../hooks/useFolderSize'
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
  /** One-shot recursive-size result for this exact folder, if any. */
  folderSize?: FolderSizeState
  onFolderSizeCancel: () => void
  /** Active listing presentation. */
  view: ViewMode
  onViewChange: (view: ViewMode) => void
}

const VIEW_BUTTONS: {
  id: ViewMode
  label: 'view.details' | 'view.list' | 'view.icons' | 'view.thumbs'
  path: string
}[] = [
  { id: 'details', label: 'view.details', path: 'M2 4h12M2 8h12M2 12h12' },
  { id: 'list', label: 'view.list', path: 'M2 4h2M6 4h8M2 8h2M6 8h8M2 12h2M6 12h8' },
  { id: 'icons', label: 'view.icons', path: 'M2.5 2.5h4v4h-4zM9.5 2.5h4v4h-4zM2.5 9.5h4v4h-4zM9.5 9.5h4v4h-4z' },
  { id: 'thumbs', label: 'view.thumbs', path: 'M2 3h12v10H2zM2 10l3.5-3 3 2.5 2.5-2 3 2.5' },
]

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
  folderSize,
  onFolderSizeCancel,
  view,
  onViewChange,
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
        {folderSize && (
          <span className="flex items-center gap-1 text-accent">
            {folderSize.error ? (
              <span className="text-red-500">{t('list.sizeFailed')}</span>
            ) : !folderSize.done ? (
              <>
                <span>{t('list.sizeCounting', { count: folderSize.files + folderSize.dirs })}</span>
                <button
                  type="button"
                  onClick={onFolderSizeCancel}
                  title={t('list.sizeCancel')}
                  className="rounded px-1 leading-none hover:bg-surface-3"
                >
                  ×
                </button>
              </>
            ) : folderSize.aborted ? (
              <span className="text-muted">
                {t('list.sizeAborted', {
                  files: folderSize.files,
                  size: formatSize(folderSize.bytes, false),
                })}
              </span>
            ) : (
              <span>
                {t('list.sizeResult', {
                  files: folderSize.files,
                  size: formatSize(folderSize.bytes, false),
                })}
              </span>
            )}
          </span>
        )}
      </div>
      <span className="flex min-w-0 items-center gap-2">
        <span className="flex items-center gap-0.5" data-view-switch>
          {VIEW_BUTTONS.map((item) => (
            <button
              key={item.id}
              type="button"
              title={t(item.label)}
              aria-label={t(item.label)}
              aria-pressed={view === item.id}
              onClick={() => onViewChange(item.id)}
              className={`rounded p-0.5 ${
                view === item.id ? 'bg-surface-3 text-fg-strong' : 'text-muted hover:bg-surface-3 hover:text-fg'
              }`}
            >
              <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.2">
                <path d={item.path} strokeLinecap="round" strokeLinejoin="round" />
              </svg>
            </button>
          ))}
        </span>
        <span className="max-w-[45vw] truncate font-mono text-[0.59375rem] text-muted-2">{path}</span>
      </span>
    </div>
  )
}