import React from 'react'
import { HeaderCell, ColDivider } from './TableHeader'
import type { Translate } from '../i18n'
import type { SortKey, SortState } from '../types'
import type { ColKey } from '../hooks/usePaneColumnResize'

export interface FileTableHeaderProps {
  t: Translate
  sort: SortState
  onSort: (key: SortKey) => void
  showSize: boolean
  showType: boolean
  showTime: boolean
  grid: string
  listMinWidth: string
  listSpan: string | undefined
  height: number
  /** Scrollbar width reclaimed so the columns line up with the rows. */
  gutter: number
  headerRef: React.RefObject<HTMLDivElement | null>
  nameWidth: number
  startColDrag: (key: ColKey, ev: React.PointerEvent<HTMLSpanElement>) => void
  resetCol: (key: ColKey) => void
  startNameDrag: (ev: React.PointerEvent<HTMLSpanElement>) => void
  onResetNameWidth: () => void
}

/** Sortable column headers for the details view. */
export function FileTableHeader({
  t,
  sort,
  onSort,
  showSize,
  showType,
  showTime,
  grid,
  listMinWidth,
  listSpan,
  height,
  gutter,
  headerRef,
  nameWidth,
  startColDrag,
  resetCol,
  startNameDrag,
  onResetNameWidth,
}: FileTableHeaderProps) {
  return (
    <div className="relative shrink-0 overflow-hidden" style={{ height, marginRight: gutter }}>
      <div
        ref={headerRef}
        className="typo-list relative grid h-full items-center border-b border-line bg-surface-2 pr-1.5 text-muted"
        style={{ gridTemplateColumns: grid, minWidth: listMinWidth, width: listSpan }}
      >
        <HeaderCell label={t('list.name')} k="name" sort={sort} onSort={onSort} align="left" />
        {showSize && (
          <div className="relative flex h-full min-w-0">
            <HeaderCell label={t('list.size')} k="size" sort={sort} onSort={onSort} align="right" />
            <ColDivider
              label={t('list.size')}
              onPointerDown={(ev) => startColDrag('size', ev)}
              onDoubleClick={() => resetCol('size')}
            />
          </div>
        )}
        {showType && (
          <div className="relative flex h-full min-w-0">
            <HeaderCell label={t('list.type')} k="type" sort={sort} onSort={onSort} align="left" />
            <ColDivider
              label={t('list.type')}
              onPointerDown={(ev) => startColDrag('type', ev)}
              onDoubleClick={() => resetCol('type')}
            />
          </div>
        )}
        {showTime && (
          <div className="relative flex h-full min-w-0">
            <HeaderCell label={t('list.mtime')} k="mtime" sort={sort} onSort={onSort} align="left" />
            <ColDivider
              label={t('list.mtime')}
              onPointerDown={(ev) => startColDrag('time', ev)}
              onDoubleClick={() => resetCol('time')}
            />
          </div>
        )}
      </div>
      {/* Pinned to the row's right edge, or to the visible edge while the list scrolls
          horizontally, so the handle is always reachable. */}
      <span
        role="separator"
        aria-orientation="vertical"
        aria-label={t('list.nameWidth')}
        title={t('list.nameWidthHint')}
        data-gutter-handle
        onPointerDown={startNameDrag}
        onDoubleClick={onResetNameWidth}
        style={{ left: nameWidth > 0 ? `min(${listMinWidth}, 100%)` : '100%' }}
        className="absolute inset-y-0 z-10 w-1.5 -translate-x-full cursor-col-resize touch-none select-none hover:bg-accent-soft/60 active:bg-accent"
      />
    </div>
  )
}