import React from 'react'
import type { SortKey, SortState } from '../types'

export function HeaderCell({
  label,
  k,
  sort,
  onSort,
  align,
}: {
  label: string
  k: SortKey
  sort: SortState
  onSort: (k: SortKey) => void
  align: 'left' | 'right'
}) {
  const active = sort.key === k
  return (
    <button
      type="button"
      onClick={() => onSort(k)}
      className={`flex h-full w-full min-w-0 items-center gap-1 px-1.5 text-[0.84em] font-medium tracking-wide text-muted hover:text-fg ${
        align === 'right' ? 'justify-end' : 'justify-start'
      }`}
    >
      <span className="truncate">{label}</span>
      <span className={`shrink-0 text-[0.64em] ${active ? 'text-blue-500' : 'text-transparent'}`}>
        {sort.dir === 'asc' ? '▲' : '▼'}
      </span>
    </button>
  )
}

/** Hit area on a column's left edge: drag resizes it, double-click restores the default. */
export function ColDivider({
  label,
  onPointerDown,
  onDoubleClick,
}: {
  label: string
  onPointerDown: (ev: React.PointerEvent<HTMLSpanElement>) => void
  onDoubleClick: () => void
}) {
  return (
    <span
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      onPointerDown={onPointerDown}
      onDoubleClick={onDoubleClick}
      className="absolute inset-y-0 left-0 z-10 w-1.5 -translate-x-1/2 cursor-col-resize touch-none select-none hover:bg-blue-400/60 active:bg-blue-500"
    />
  )
}
