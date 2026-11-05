import React from 'react'
import { FileIcon } from '../icons'
import { GitItemBadge } from './GitBadges'
import { EntryStateBadges, entryStateTitle, entryStateTone } from './EntryStateBadge'
import { colorRuleFor, colorRuleStyle } from '../colorRules'
import type { ColorRule } from '../persist'
import type { Entry } from '../types'
import type { GitBadges as GitBadgesData } from '../git'
import type { Translate } from '../i18n'
import type { MenuPoint } from '../PaneView'
import { entryPath, formatSize, formatTime, isDir } from '../paths'

const typeOf = (name: string, dir: boolean, t: Translate): string => {
  if (dir) return t('list.typeFolder')
  const dot = name.lastIndexOf('.')
  return dot > 0 && dot < name.length - 1 ? name.slice(dot + 1).toUpperCase() : t('list.typeFile')
}

export interface FileTableRowProps {
  entry: Entry
  index: number
  tabPath: string
  rowHeight: number
  startOffset: number
  grid: string
  isSelected: boolean
  listScale: number
  iconPx: number
  gitBadges: GitBadgesData
  colorRules: ColorRule[]
  t: Translate
  showSize: boolean
  showType: boolean
  showTime: boolean
  newTabOnMiddleClick: boolean
  selectRow: (index: number, e: React.MouseEvent) => void
  onRowDragStart: (entry: Entry, e: React.PointerEvent) => void
  onOpenInNewTab: (path: string) => void
  onRowContextMenu: (entry: Entry, point: MenuPoint) => void
  onDoubleClick: () => void
  pointOf: (ev: React.MouseEvent) => MenuPoint
  isRowSelected: boolean
}

export function FileTableRow({
  entry,
  index,
  tabPath,
  rowHeight,
  startOffset,
  grid,
  isSelected,
  listScale,
  iconPx,
  gitBadges,
  colorRules,
  t,
  showSize,
  showType,
  showTime,
  newTabOnMiddleClick,
  selectRow,
  onRowDragStart,
  onOpenInNewTab,
  onRowContextMenu,
  onDoubleClick,
  pointOf,
  isRowSelected,
}: FileTableRowProps) {
  const dir = isDir(entry.attrs)
  const fullPath = entryPath(tabPath, entry)
  const stateTitle = entryStateTitle(entry.name, entry.attrs, t)
  const stateTone = entryStateTone(entry.name, entry.attrs)
  const nameTone = stateTone || (dir ? 'text-fg-strong' : '')
  const rule = colorRuleFor(entry.name, dir, colorRules)
  const ruleStyle = colorRuleStyle(rule)

  return (
    <div
      key={`${entry.name}-${index}`}
      data-row={entry.name}
      data-dir={dir ? '1' : '0'}
      data-drop-dir={dir ? fullPath : undefined}
      onPointerDown={(e) => {
        if (e.button === 0 && (e.ctrlKey || e.metaKey) && dir && newTabOnMiddleClick) {
          e.preventDefault()
          onOpenInNewTab(fullPath)
          return
        }
        if (e.button !== 0) return
        selectRow(index, e)
        onRowDragStart(entry, e)
      }}
      onAuxClick={(e) => {
        if (e.button !== 1 || !dir || !newTabOnMiddleClick) return
        e.preventDefault()
        onOpenInNewTab(fullPath)
      }}
      onContextMenu={(e) => {
        e.preventDefault()
        e.stopPropagation()
        if (!isRowSelected) selectRow(index, e)
        onRowContextMenu(entry, pointOf(e))
      }}
      onDoubleClick={onDoubleClick}
      className={`absolute left-0 top-0 grid w-full items-center border-b border-row-line pr-1.5 ${
        isSelected ? 'bg-sel text-fg-strong' : 'hover:bg-surface-3'
      }`}
      style={{ height: rowHeight, transform: `translateY(${startOffset}px)`, gridTemplateColumns: grid }}
    >
      <div className="flex min-w-0 items-center gap-1.5 pl-1.5">
        <span className="relative shrink-0">
          <FileIcon name={entry.name} dir={dir} path={fullPath} scale={listScale} size={iconPx} />
          <EntryStateBadges name={entry.name} attrs={entry.attrs} t={t} />
          <GitItemBadge name={entry.name} dir={dir} badges={gitBadges} iconSize={iconPx} t={t} />
        </span>
        <span
          className={`truncate ${dir ? 'font-medium' : ''} ${nameTone} ${rule?.background ? 'rounded px-1' : ''}`}
          style={ruleStyle}
          title={stateTitle || undefined}
        >
          {entry.name}
        </span>
      </div>
      {showSize && (
        <div className="tabular whitespace-nowrap pr-1.5 text-right text-[0.88em] text-muted">
          {formatSize(entry.size, dir)}
        </div>
      )}
      {showType && (
        <div className="truncate whitespace-nowrap pl-1.5 text-[0.88em] text-muted">
          {typeOf(entry.name, dir, t)}
        </div>
      )}
      {showTime && (
        <div className="tabular whitespace-nowrap pl-1.5 text-[0.88em] text-muted">
          {formatTime(entry.mtime)}
        </div>
      )}
    </div>
  )
}
