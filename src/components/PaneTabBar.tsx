import React from 'react'
import type { PaneState, TabState } from '../workspace'
import type { Translate } from '../i18n'
import type { MenuPoint } from '../PaneView'

export interface PaneTabBarProps {
  pane: PaneState
  tabs: Record<string, TabState>
  active: boolean
  tabBarHeight: number
  t: Translate
  stripRef: React.RefObject<HTMLDivElement | null>
  stripOverflow: boolean
  tabListOpen: boolean
  setTabListOpen: React.Dispatch<React.SetStateAction<boolean>>
  onActivateTab: (tabId: string) => void
  onCloseTab: (tabId: string) => void
  onNewTab: () => void
  onTabDragStart: (tabId: string, ev: React.PointerEvent) => void
  onTabContextMenu: (tabId: string, point: MenuPoint) => void
  pointOf: (ev: React.MouseEvent) => MenuPoint
  draggingTabId: string | null
}

export function PaneTabBar({
  pane,
  tabs,
  active,
  tabBarHeight,
  t,
  stripRef,
  stripOverflow,
  tabListOpen,
  setTabListOpen,
  onActivateTab,
  onCloseTab,
  onNewTab,
  onTabDragStart,
  onTabContextMenu,
  pointOf,
  draggingTabId,
}: PaneTabBarProps) {
  return (
    <div
      data-tabbar
      onDoubleClick={(ev) => {
        if ((ev.target as HTMLElement).closest('[data-tab]')) return
        onNewTab()
      }}
      style={{ height: tabBarHeight }}
      className={`typo-tab relative flex shrink-0 items-stretch gap-0.5 border-b border-line px-1 pt-[2px] ${
        active ? 'bg-surface' : 'bg-surface-2'
      }`}
    >
      {stripOverflow && (
        <button
          type="button"
          data-tab-scroll="left"
          title={t('tab.scrollLeft')}
          aria-label={t('tab.scrollLeft')}
          onClick={() => stripRef.current?.scrollBy({ left: -160, behavior: 'smooth' })}
          className="grid w-[1.5em] shrink-0 place-items-center rounded text-muted-2 hover:bg-surface-3 hover:text-fg"
        >
          ‹
        </button>
      )}
      <div ref={stripRef} data-tab-strip className="no-bar flex min-w-0 flex-1 items-stretch gap-0.5 overflow-x-auto">
        {pane.tabIds.map((id) => {
          const tabState = tabs[id]
          if (!tabState) return null
          const isTabActive = id === pane.activeTabId
          const isDragging = id === draggingTabId
          return (
            <div
              key={id}
              data-tab={id}
              onClick={() => onActivateTab(id)}
              onPointerDown={(ev) => {
                if (ev.button === 0) onTabDragStart(id, ev)
              }}
              onContextMenu={(ev) => {
                ev.preventDefault()
                onTabContextMenu(id, pointOf(ev))
              }}
              onAuxClick={(ev) => {
                if (ev.button !== 1) return
                ev.preventDefault()
                onCloseTab(id)
              }}
              title={tabState.path || t('sidebar.thisPc')}
              className={`group flex min-w-0 max-w-[12rem] cursor-default items-center gap-1 rounded-t px-2 text-[0.84em] select-none ${
                isTabActive
                  ? active
                    ? 'border-t-2 border-t-blue-500 bg-surface-2 font-medium text-fg-strong'
                    : 'border-t-2 border-t-blue-400/50 bg-surface-2 text-fg-strong'
                  : 'text-muted hover:bg-surface-3 hover:text-fg'
              } ${isDragging ? 'opacity-40' : ''}`}
            >
              {tabState.pinned && (
                <span className="shrink-0 text-[0.625rem] text-blue-500" title={t('tab.pinned')}>
                  ★
                </span>
              )}
              <span className="max-w-[9.375rem] truncate">
                {tabState.path
                  ? (tabState.path.split('\\').filter(Boolean).pop() ?? tabState.path)
                  : t('sidebar.thisPc')}
              </span>
              <button
                type="button"
                onPointerDown={(ev) => ev.stopPropagation()}
                onClick={(ev) => {
                  ev.stopPropagation()
                  onCloseTab(id)
                }}
                className="grid h-[1.33em] w-[1.33em] shrink-0 place-items-center rounded text-muted-2 hover:bg-surface-3 hover:text-fg"
                title={t('tab.close')}
                aria-label={t('tab.close')}
              >
                ×
              </button>
            </div>
          )
        })}
      </div>
      {stripOverflow && (
        <>
          <button
            type="button"
            data-tab-scroll="right"
            title={t('tab.scrollRight')}
            aria-label={t('tab.scrollRight')}
            onClick={() => stripRef.current?.scrollBy({ left: 160, behavior: 'smooth' })}
            className="grid w-[1.5em] shrink-0 place-items-center rounded text-muted-2 hover:bg-surface-3 hover:text-fg"
          >
            ›
          </button>
          <button
            type="button"
            data-tab-overflow="1"
            title={t('tab.overflow')}
            aria-label={t('tab.overflow')}
            aria-expanded={tabListOpen}
            onClick={() => setTabListOpen((open) => !open)}
            className="grid w-[1.5em] shrink-0 place-items-center rounded text-muted-2 hover:bg-surface-3 hover:text-fg"
          >
            ⌄
          </button>
        </>
      )}
      <button
        type="button"
        title={t('tab.new')}
        aria-label={t('tab.new')}
        onClick={onNewTab}
        className="grid h-[1.67em] w-[1.67em] shrink-0 place-items-center self-center rounded text-muted hover:bg-surface-3 hover:text-fg"
      >
        <svg viewBox="0 0 16 16" className="h-[1.17em] w-[1.17em]" fill="none" stroke="currentColor" strokeWidth="1.6">
          <path d="M8 3.5v9M3.5 8h9" strokeLinecap="round" />
        </svg>
      </button>
      {tabListOpen && (
        <>
          <div className="fixed inset-0 z-40" onMouseDown={() => setTabListOpen(false)} />
          <div
            data-tab-list
            className="absolute right-1 top-full z-50 max-h-[50vh] w-[16rem] overflow-auto rounded-md border border-line bg-surface py-1 shadow-xl"
          >
            {pane.tabIds.map((id) => {
              const state = tabs[id]
              if (!state) return null
              return (
                <button
                  key={id}
                  type="button"
                  onClick={() => {
                    onActivateTab(id)
                    setTabListOpen(false)
                  }}
                  className={`flex w-full items-center gap-1.5 px-2 py-1 text-left text-[0.75rem] hover:bg-surface-3 ${
                    id === pane.activeTabId ? 'bg-sel text-fg-strong' : 'text-fg'
                  }`}
                >
                  <span className="w-[1em] shrink-0 text-center text-[0.625rem] text-muted-2">
                    {state.pinned ? '★' : ''}
                  </span>
                  <span className="min-w-0 flex-1 truncate">{state.path || t('sidebar.thisPc')}</span>
                </button>
              )
            })}
          </div>
        </>
      )}
    </div>
  )
}
