import React, { useCallback, useEffect } from 'react'
import type { Translate } from '../i18n'
import type { Drive } from '../types'
import { SidebarItem } from './SidebarItem'
import { SidebarTree } from '../SidebarTree'
import type { QuickPlaceEntry } from '../hooks/usePaneContextMenus'
import type { MenuPoint } from '../PaneView'
import { pathKey } from '../paths'
import { centerOffset } from '../scroll'
export interface SidebarPanelProps {
  sidebarRef: React.RefObject<HTMLElement | null>
  sidebarWidth: number
  t: Translate
  quickPlaces: QuickPlaceEntry[]
  treeSelected: string
  treeFocus?: string
  openTreePath: (path: string) => void
  openPlaceMenu: (place: QuickPlaceEntry, point: MenuPoint) => void
  driveList: Drive[]
  treeOpen: Record<string, string>
  treeNodes: Record<string, string[]>
  treeTruncated: Record<string, boolean>
  activePath: string
  toggleTree: (path: string) => void
  openTreeContextMenu: (path: string, point: { clientX: number; clientY: number }) => void
  pointOf: (ev: React.MouseEvent) => MenuPoint
}

export function SidebarPanel({
  sidebarRef,
  sidebarWidth,
  t,
  quickPlaces,
  treeSelected,
  treeFocus,
  openTreePath,
  openPlaceMenu,
  driveList,
  treeOpen,
  treeNodes,
  treeTruncated,
  activePath,
  toggleTree,
  openTreeContextMenu,
  pointOf,
}: SidebarPanelProps) {
  const revealTreeRow = useCallback(
    (path: string, force: boolean) => {
      const scroller = sidebarRef.current
      if (!scroller) return
      const row = scroller.querySelector<HTMLElement>(`[data-tree-key="${CSS.escape(pathKey(path))}"]`)
      if (!row) return
      const box = scroller.getBoundingClientRect()
      const rect = row.getBoundingClientRect()
      const margin = 16
      const visible = rect.top >= box.top + margin && rect.bottom <= box.bottom - margin
      if (!force && visible) return
      scroller.scrollTo({
        top: centerOffset(rect.top, rect.height, box.top, box.height, scroller.scrollTop),
        behavior: 'smooth',
      })
    },
    [sidebarRef],
  )

  useEffect(() => {
    if (!treeFocus) return
    const timer = window.setTimeout(() => revealTreeRow(treeFocus, false), 80)
    return () => window.clearTimeout(timer)
  }, [revealTreeRow, treeFocus])

  useEffect(() => {
    if (!treeSelected) return
    const timer = window.setTimeout(() => revealTreeRow(treeSelected, false), 40)
    return () => window.clearTimeout(timer)
  }, [revealTreeRow, treeSelected])
  return (
    <aside
      ref={sidebarRef}
      style={{
        width: sidebarWidth,
        maxWidth: 'calc(50vw - 1px)',
      }}
      className="flex shrink-0 flex-col overflow-x-hidden border-r border-line bg-surface"
    >
      <section className="border-b border-line px-2 py-2">
        <h2 className="px-2 pb-1 text-[0.625rem] font-semibold tracking-wider text-muted-2">
          {t('sidebar.quickAccess')}
        </h2>
        {quickPlaces.map((place) => (
          <SidebarItem
            key={place.key}
            label={place.label}
            hint={place.custom ? t('sidebar.pinPlace') : undefined}
            active={place.path === treeSelected}
            dropDir={place.path}
            onClick={() => openTreePath(place.path)}
            onContextMenu={(ev) => openPlaceMenu(place, pointOf(ev))}
            icon={
              <svg viewBox="0 0 16 16" className="h-4 w-4 text-blue-500" fill="currentColor">
                <path d="M1.5 3.6c0-.6.5-1.1 1.1-1.1h3.2c.4 0 .7.15.95.42l.85.93h6.3c.6 0 1.1.5 1.1 1.1v7.4c0 .6-.5 1.1-1.1 1.1H2.6c-.6 0-1.1-.5-1.1-1.1V3.6Z" />
              </svg>
            }
          />
        ))}
      </section>
      <section className="px-2 py-2">
        <h2 className="px-2 pb-1 text-[0.625rem] font-semibold tracking-wider text-muted-2">
          {t('sidebar.thisPc')}
        </h2>
        <SidebarTree
          drives={driveList}
          open={treeOpen}
          nodes={treeNodes}
          truncated={treeTruncated}
          truncatedLabel={t('tree.truncated')}
          selectedPath={treeSelected}
          currentPath={activePath}
          onToggle={toggleTree}
          onOpen={openTreePath}
          onContextMenu={(path, point) => openTreeContextMenu(path, point)}
        />
      </section>
    </aside>
  )
}
