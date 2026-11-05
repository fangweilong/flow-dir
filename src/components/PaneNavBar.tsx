import React from 'react'
import { AddressBar, type AddressBarHandle } from './AddressBar'
import type { Translate } from '../i18n'
import type { Drive } from '../types'
import type { ShortcutId } from '../shortcuts'
import type { TabState } from '../workspace'
import type { MenuPoint } from '../PaneView'

export interface PaneNavBarProps {
  addressBarRef: React.RefObject<AddressBarHandle | null>
  tab: TabState | undefined
  keys: Record<ShortcutId, string>
  t: Translate
  location: Drive | undefined
  root: boolean
  parts: { label: string; path: string }[]
  upTarget: string
  navButtonStyle: React.CSSProperties
  navFieldStyle: React.CSSProperties
  height: number
  hidden: boolean
  /** Width-dependent: the filter input folds into a chip when the bar is narrow. */
  showFilter: boolean
  filter: string
  onFilterChange: (text: string) => void
  onToggleHidden: () => void
  onHistoryMove: (tabId: string, delta: number) => void
  onNavigate: (tabId: string, path: string) => void
  onDriveMenu: (point: MenuPoint) => void
  onCopyPath: (path: string) => void
  onPathInvalid: (path: string) => void
  onOpenInNewTab: (path: string) => void
  newTabOnMiddleClick: boolean
}

/** Address bar, filter field and the hidden-items toggle above one pane's list. */
export function PaneNavBar({
  addressBarRef,
  tab,
  keys,
  t,
  location,
  root,
  parts,
  upTarget,
  navButtonStyle,
  navFieldStyle,
  height,
  hidden,
  showFilter,
  filter,
  onFilterChange,
  onToggleHidden,
  onHistoryMove,
  onNavigate,
  onDriveMenu,
  onCopyPath,
  onPathInvalid,
  onOpenInNewTab,
  newTabOnMiddleClick,
}: PaneNavBarProps) {
  return (
    <div
      className="typo-nav flex shrink-0 items-center gap-0.5 border-b border-line bg-surface px-1"
      style={{ height }}
    >
      <AddressBar
        ref={addressBarRef}
        tab={tab}
        keys={keys}
        t={t}
        location={location}
        root={root}
        parts={parts}
        upTarget={upTarget}
        navButtonStyle={navButtonStyle}
        navFieldStyle={navFieldStyle}
        onHistoryMove={onHistoryMove}
        onNavigate={onNavigate}
        onDriveMenu={onDriveMenu}
        onCopyPath={onCopyPath}
        onPathInvalid={onPathInvalid}
        onOpenInNewTab={onOpenInNewTab}
        newTabOnMiddleClick={newTabOnMiddleClick}
      />

      {showFilter && !root && (
        <input
          value={filter}
          onChange={(e) => onFilterChange(e.target.value)}
          placeholder={t('list.filter')}
          style={navFieldStyle}
          className="w-[5.25rem] shrink-0 rounded border border-line bg-surface-2 px-1.5 text-[0.92em] outline-none placeholder:text-muted-2 focus:border-blue-400 focus:bg-surface"
        />
      )}
      {!showFilter && !root && filter && (
        // The input is gone at this width, but the filter still decides what the list
        // shows: keep a chip so it is never invisible and never sticky.
        <button
          type="button"
          data-filter-clear
          onClick={() => onFilterChange('')}
          title={t('list.filterActive', { query: filter })}
          style={navFieldStyle}
          className="ml-0.5 flex max-w-[5rem] shrink-0 items-center gap-0.5 rounded border border-line bg-surface-3 px-1 text-[0.85em] text-fg"
        >
          <span className="truncate">{filter}</span>
          <span className="shrink-0 text-muted">×</span>
        </button>
      )}
      <button
        type="button"
        onClick={onToggleHidden}
        title={hidden ? t('list.hideHidden') : t('list.showHidden')}
        style={navButtonStyle}
        className={`grid shrink-0 place-items-center rounded hover:bg-surface-3 ${
          hidden ? 'text-blue-500' : 'text-muted-2'
        }`}
      >
        <svg viewBox="0 0 16 16" className="h-[1.17em] w-[1.17em]" fill="none" stroke="currentColor" strokeWidth="1.4">
          <path d="M1.5 8s2.4-4 6.5-4 6.5 4 6.5 4-2.4 4-6.5 4-6.5-4-6.5-4Z" />
          <circle cx="8" cy="8" r="1.8" />
        </svg>
      </button>
    </div>
  )
}