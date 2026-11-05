import { describeError, type Translate } from '../../i18n'
import type { CompareCount, CompareEntry, CompareHashResult, CompareStatus } from '../../types'
import { FileIcon } from '../../icons'
import { EntryStateBadges, entryStateTitle, entryStateTone } from '../EntryStateBadge'
import { colorRuleFor, colorRuleStyle } from '../../colorRules'
import type { ColorRule } from '../../persist'
import { formatSize, joinPath } from '../../paths'

export interface CompareTableViewProps {
  visible: CompareEntry[]
  leftPath: string
  rightPath: string
  counts: Record<string, CompareCount>
  hashResults: Record<string, CompareHashResult>
  onEnter: (rel: string) => void
  iconScale: number
  iconPx?: number
  colorRules: ColorRule[]
  statusLabel: (status: CompareStatus) => string
  statusClass: (status: CompareStatus) => string
  statusMark: (status: CompareStatus) => string
  fmtTime: (ms: number) => string
  t: Translate
}

export function CompareTableView({
  visible,
  leftPath,
  rightPath,
  counts,
  hashResults,
  onEnter,
  iconScale,
  iconPx,
  colorRules,
  statusLabel,
  statusClass,
  statusMark,
  fmtTime,
  t,
}: CompareTableViewProps) {
  const sideCell = (entry: CompareEntry, side: 'left' | 'right') => {
    const meta = entry[side]
    if (!meta) {
      return <div className="min-w-0 bg-surface-2/40 px-2 py-1 text-muted-2/40">—</div>
    }
    const root = side === 'left' ? leftPath : rightPath
    const rule = colorRuleFor(entry.name, meta.isDir, colorRules)
    const canEnter = !!entry.left?.isDir && !!entry.right?.isDir
    const count = counts[entry.rel]
    const hash = hashResults[entry.rel]
    const tint =
      entry.status === 'different'
        ? 'bg-warning-wash/60 dark:bg-warning-night/25'
        : entry.status === 'left' && side === 'left'
          ? 'bg-rose-50/60 dark:bg-rose-950/25'
          : entry.status === 'right' && side === 'right'
            ? 'bg-emerald-50/60 dark:bg-emerald-950/25'
            : ''
    return (
      <div className={`flex min-w-0 items-center gap-1.5 px-2 py-1 ${tint}`}>
        <span className="relative shrink-0">
          <FileIcon
            name={entry.name}
            dir={meta.isDir}
            path={joinPath(root, entry.rel)}
            scale={iconScale}
            size={iconPx}
          />
          <EntryStateBadges name={entry.name} attrs={meta.attrs} t={t} />
        </span>
        <span
          className={`truncate ${
            entryStateTone(entry.name, meta.attrs) ||
            (entry.status === 'same' ? 'text-muted' : 'text-fg-strong')
          } ${rule?.background ? 'rounded px-1' : ''}`}
          style={colorRuleStyle(rule)}
          title={entryStateTitle(entry.name, meta.attrs, t) || entry.rel}
        >
          {entry.name}
        </span>
        {side === 'left' && count && (
          <span
            className={`shrink-0 text-[0.625rem] ${
              count.error ? 'text-rose-600 dark:text-rose-400' : 'text-muted-2'
            }`}
            title={count.error ? describeError(t, count.error) : t('compare.countsHint')}
          >
            {/* A failed recursive count must not read as "0/0/0" — that looks like an
                empty folder instead of a level that could not be scanned. */}
            {count.error ? '!' : `${count.leftOnly}/${count.different}/${count.rightOnly}`}
          </span>
        )}
        {side === 'left' && canEnter && (
          <button
            type="button"
            onClick={() => onEnter(entry.rel)}
            className="shrink-0 rounded border border-line px-1 text-[0.625rem] text-muted hover:text-fg"
          >
            {t('compare.enter')}
          </button>
        )}
        {side === 'right' && hash && (
          <span
            title={`${hash.leftHash ?? ''} / ${hash.rightHash ?? ''}${hash.error ? ` — ${hash.error}` : ''}`}
            className={`shrink-0 font-mono ${hash.equal ? 'text-emerald-500' : 'text-rose-500'}`}
          >
            {hash.equal ? '=' : '≠'}
          </span>
        )}
        <span className="ml-auto shrink-0 tabular text-muted-2">{formatSize(meta.size, meta.isDir)}</span>
        <span className="shrink-0 text-muted-2">{fmtTime(meta.mtime)}</span>
      </div>
    )
  }

  return (
    <div className="min-w-[40rem] text-[0.6875rem]">
      <div className="sticky top-0 z-10 grid grid-cols-[1fr_2.25rem_1fr] border-b border-line bg-surface-2 text-[0.625rem] text-muted-2">
        <div className="truncate px-2 py-1" title={leftPath}>
          {leftPath}
        </div>
        <div className="border-x border-line" />
        <div className="truncate px-2 py-1" title={rightPath}>
          {rightPath}
        </div>
      </div>
      {visible.map((entry) => {
        const canEnter = !!entry.left?.isDir && !!entry.right?.isDir
        return (
          <div
            key={entry.rel}
            className="grid grid-cols-[1fr_2.25rem_1fr] border-b border-row-line hover:bg-surface-3"
            onDoubleClick={() => canEnter && onEnter(entry.rel)}
          >
            {sideCell(entry, 'left')}
            <div
              className={`grid place-items-center border-x border-row-line font-mono text-[0.8125rem] font-semibold ${statusClass(entry.status)}`}
              title={statusLabel(entry.status)}
            >
              {statusMark(entry.status)}
            </div>
            {sideCell(entry, 'right')}
          </div>
        )
      })}
    </div>
  )
}
