import React from 'react'
import { FileTableRow } from './FileTableRow'
import { FileGridCell, type GridMode } from './FileGridCell'
import type { Entry, ListData } from '../types'
import type { Translate } from '../i18n'
import type { ColorRule } from '../persist'
import type { GitBadges } from '../git'
import type { ViewMode } from '../workspace'
import type { MenuPoint } from '../PaneView'

export interface FileListBodyProps {
  view: Entry[]
  list: ListData
  viewMode: ViewMode
  columns: number
  rowHeight: number
  cellHeight: number
  listMinWidth: string
  listSpan: string | undefined
  grid: string
  listScale: number
  iconPx: number
  showSize: boolean
  showType: boolean
  showTime: boolean
  gitBadges: GitBadges
  colorRules: ColorRule[]
  t: Translate
  tabPath: string
  virtualizer: {
    getTotalSize: () => number
    getVirtualItems: () => { index: number; start: number }[]
  }
  newTabOnMiddleClick: boolean
  selectRow: (index: number, e: React.MouseEvent) => void
  onRowDragStart: (entry: Entry, ev: React.PointerEvent) => void
  onOpenInNewTab: (path: string) => void
  onRowContextMenu: (entry: Entry, point: MenuPoint) => void
  onActivate: (entry: Entry) => void
  pointOf: (ev: React.MouseEvent) => MenuPoint
  loading: boolean
  filter: string
}

/**
 * Virtualized rows for the details table and the grid views, plus the loading, empty and
 * filtered placeholders. The scroll container and its chrome stay in `PaneView`.
 */
export function FileListBody({
  view,
  list,
  viewMode,
  columns,
  rowHeight,
  cellHeight,
  listMinWidth,
  listSpan,
  grid,
  listScale,
  iconPx,
  showSize,
  showType,
  showTime,
  gitBadges,
  colorRules,
  t,
  tabPath,
  virtualizer,
  newTabOnMiddleClick,
  selectRow,
  onRowDragStart,
  onOpenInNewTab,
  onRowContextMenu,
  onActivate,
  pointOf,
  loading,
  filter,
}: FileListBodyProps) {
  const gridMode: GridMode | null = viewMode === 'details' ? null : viewMode
  return (
    <>
      {viewMode === 'details' && (
        <div style={{ height: virtualizer.getTotalSize(), position: 'relative', minWidth: listMinWidth, width: listSpan }}>
          {virtualizer.getVirtualItems().map((vi) => {
            const entry = view[vi.index]
            if (!entry) return null
            return (
              <FileTableRow
                key={`${entry.name}-${vi.index}`}
                entry={entry}
                index={vi.index}
                tabPath={tabPath}
                rowHeight={rowHeight}
                startOffset={vi.start}
                grid={grid}
                isSelected={list.selected.has(entry.name)}
                listScale={listScale}
                iconPx={iconPx}
                gitBadges={gitBadges}
                colorRules={colorRules}
                t={t}
                showSize={showSize}
                showType={showType}
                showTime={showTime}
                newTabOnMiddleClick={newTabOnMiddleClick}
                selectRow={selectRow}
                onRowDragStart={onRowDragStart}
                onOpenInNewTab={onOpenInNewTab}
                onRowContextMenu={onRowContextMenu}
                onDoubleClick={() => onActivate(entry)}
                pointOf={pointOf}
                isRowSelected={list.selected.has(entry.name)}
              />
            )
          })}
        </div>
      )}

      {gridMode && (
        <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
          {virtualizer.getVirtualItems().map((vi) => {
            const start = vi.index * columns
            return (
              <div
                key={vi.index}
                className="absolute left-0 top-0 grid w-full"
                style={{
                  height: cellHeight,
                  transform: `translateY(${vi.start}px)`,
                  gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))`,
                }}
              >
                {Array.from({ length: columns }, (_, col) => {
                  const index = start + col
                  const entry = view[index]
                  if (!entry) return <div key={`empty-${col}`} />
                  return (
                    <FileGridCell
                      key={`${entry.name}-${index}`}
                      entry={entry}
                      index={index}
                      mode={gridMode}
                      tabPath={tabPath}
                      cellHeight={cellHeight}
                      iconPx={iconPx}
                      listScale={listScale}
                      isSelected={list.selected.has(entry.name)}
                      gitBadges={gitBadges}
                      colorRules={colorRules}
                      t={t}
                      newTabOnMiddleClick={newTabOnMiddleClick}
                      selectRow={selectRow}
                      onRowDragStart={onRowDragStart}
                      onOpenInNewTab={onOpenInNewTab}
                      onRowContextMenu={onRowContextMenu}
                      onDoubleClick={() => onActivate(entry)}
                      pointOf={pointOf}
                    />
                  )
                })}
              </div>
            )
          })}
        </div>
      )}

      {!list.error && !view.length && (
        <div className="grid h-full place-items-center text-center text-muted-2">
          {loading ? (
            <div className="flex flex-col items-center gap-2">
              <div className="h-4 w-4 animate-spin rounded-full border-2 border-line border-t-blue-500" />
              <div className="text-[0.88em]">
                {t('list.loading')} {list.scanned > 0 && t('list.scanned', { count: list.scanned })}
              </div>
            </div>
          ) : filter ? (
            <div className="text-[0.88em]">{t('list.noMatch', { query: filter })}</div>
          ) : (
            <div className="text-[0.88em]">{t('list.empty')}</div>
          )}
        </div>
      )}
    </>
  )
}