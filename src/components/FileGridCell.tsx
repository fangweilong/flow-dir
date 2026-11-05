import React from 'react'
import { FileIcon } from '../icons'
import { EntryStateBadges, entryStateTitle, entryStateTone } from './EntryStateBadge'
import { GitItemBadge } from './GitBadges'
import { colorRuleFor, colorRuleStyle } from '../colorRules'
import type { ColorRule } from '../persist'
import type { Entry } from '../types'
import type { GitBadges as GitBadgesData } from '../git'
import type { Translate } from '../i18n'
import type { MenuPoint } from '../PaneView'
import { entryPath, isDir } from '../paths'

export type GridMode = 'list' | 'icons' | 'thumbs'

export interface FileGridCellProps {
  entry: Entry
  index: number
  mode: GridMode
  tabPath: string
  cellHeight: number
  iconPx: number
  listScale: number
  isSelected: boolean
  gitBadges: GitBadgesData
  colorRules: ColorRule[]
  t: Translate
  newTabOnMiddleClick: boolean
  selectRow: (index: number, e: React.MouseEvent) => void
  onRowDragStart: (entry: Entry, ev: React.PointerEvent) => void
  onOpenInNewTab: (path: string) => void
  onRowContextMenu: (entry: Entry, point: MenuPoint) => void
  onDoubleClick: () => void
  pointOf: (ev: React.MouseEvent) => MenuPoint
}

/** One tile in the list / icons / thumbnails views. */
export function FileGridCell({
  entry,
  index,
  mode,
  tabPath,
  cellHeight,
  iconPx,
  listScale,
  isSelected,
  gitBadges,
  colorRules,
  t,
  newTabOnMiddleClick,
  selectRow,
  onRowDragStart,
  onOpenInNewTab,
  onRowContextMenu,
  onDoubleClick,
  pointOf,
}: FileGridCellProps) {
  const dir = isDir(entry.attrs)
  const fullPath = entryPath(tabPath, entry)
  const rule = colorRuleFor(entry.name, dir, colorRules)
  const tone = entryStateTone(entry.name, entry.attrs) || (dir ? 'text-fg-strong' : 'text-fg')
  const stateTitle = entryStateTitle(entry.name, entry.attrs, t)
  const thumb = mode === 'thumbs' && !dir
  const iconSize = mode === 'list' ? iconPx : mode === 'thumbs' ? 96 : Math.max(32, iconPx * 2)

  return (
    <div
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
        if (!isSelected) selectRow(index, e)
        onRowContextMenu(entry, pointOf(e))
      }}
      onDoubleClick={onDoubleClick}
      style={{ height: cellHeight }}
      className={`flex min-w-0 cursor-default select-none overflow-hidden rounded ${
        isSelected ? 'bg-sel' : 'hover:bg-surface-3'
      } ${mode === 'list' ? 'items-center gap-1.5 px-1.5' : 'flex-col items-center gap-1 px-1 pt-2'}`}
      title={stateTitle || entry.name}
    >
      <span className={`relative shrink-0 ${thumb ? 'flex items-center justify-center' : ''}`}>
        <FileIcon
          name={entry.name}
          dir={dir}
          path={fullPath}
          scale={listScale}
          size={iconSize}
          thumb={thumb}
        />
        <EntryStateBadges name={entry.name} attrs={entry.attrs} t={t} />
        <GitItemBadge name={entry.name} dir={dir} badges={gitBadges} iconSize={iconPx} t={t} />
      </span>
      <span
        style={colorRuleStyle(rule)}
        className={`min-w-0 ${mode === 'list' ? 'truncate' : 'line-clamp-2 w-full text-center leading-tight'} ${
          rule?.background ? 'rounded px-1' : ''
        } ${tone}`}
      >
        {entry.name}
      </span>
    </div>
  )
}