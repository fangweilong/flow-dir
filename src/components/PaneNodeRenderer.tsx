import React from 'react'
import { PaneView } from '../PaneView'
import type { MenuPoint } from '../PaneView'
import { SplitView } from './SplitView'
import type { Drive, Entry, GitStatus, Lang, ListData, SortState } from '../types'
import type { Node, Workspace } from '../workspace'
import type { Settings } from '../persist'
import type { ShortcutId } from '../shortcuts'
import type { DragState } from '../hooks/useWorkspaceLayout'
import { pathKey } from '../paths'

export interface PaneNodeRendererProps {
  node: Node
  ws: Workspace
  listData: Record<string, ListData>
  gitByDir: Record<string, GitStatus>
  drag: DragState | null
  lang: Lang
  settings: Settings
  driveList: Drive[]
  keys: Record<ShortcutId, string>
  historyEpoch: number
  registerPane: (id: string, el: HTMLDivElement | null) => void
  focusPane: (paneId: string) => void
  newTab: (paneId: string) => void
  openTabAt: (paneId: string, path: string) => void
  closeTab: (paneId: string, tabId: string) => void
  activateTab: (paneId: string, tabId: string) => void
  openTarget: (targetPath: string) => void
  navigate: (tabId: string, path: string) => void
  historyMove: (tabId: string, delta: number) => void
  startTabDrag: (tabId: string, ev: React.PointerEvent) => void
  setPaneSort: (paneId: string, sort: SortState) => void
  toggleHidden: (paneId: string) => void
  copySelection: (paneId: string, cut: boolean) => void
  copyCurrentPath: (path: string) => void
  reportInvalidPath: (path: string) => void
  openRowMenu: (paneId: string, entry: Entry, point: MenuPoint) => void
  startRowDrag: (paneId: string, entry: Entry, ev: React.PointerEvent) => void
  openBlankMenu: (paneId: string, point: MenuPoint) => void
  openTabMenu: (paneId: string, tabId: string, point: MenuPoint) => void
  openDriveMenu: (paneId: string, point: MenuPoint) => void
  openDriveRowMenu: (drive: Drive, point: MenuPoint) => void
  openProperties: (paneId: string, names: string[]) => void
  startRename: (paneId: string, name: string) => void
  startRenameMany: (paneId: string, names: string[]) => void
  deleteEntries: (paneId: string, names: string[], permanent: boolean) => void
  pasteInto: (paneId: string) => void
  manualRefresh: () => void
  retryTab: (tabId: string) => void
  update: (tabId: string, fn: (d: ListData) => ListData) => void
  setColWidths: (size: number, time: number, type: number, capacity: number, free: number) => void
  setNameWidth: (width: number) => void
  applyRatio: (splitId: string, ratio: number) => void
}

export function PaneNodeRenderer(props: PaneNodeRendererProps) {
  const { node, ws, listData, gitByDir, drag, lang, settings, driveList, keys } = props

  if (node.t === 'pane') {
    const pane = ws.panes[node.id]
    if (!pane) return null
    return (
      <div
        key={node.id}
        ref={(el) => props.registerPane(node.id, el)}
        className="flex min-h-0 min-w-0 flex-1 flex-col"
      >
        <PaneView
          pane={pane}
          tabs={ws.tabs}
          data={listData[pane.activeTabId]}
          git={gitByDir[pathKey(ws.tabs[pane.activeTabId]?.path ?? '')] ?? null}
          active={ws.activePaneId === pane.id}
          lang={lang}
          iconSize={settings.iconSize}
          tabFontSize={settings.tabFontSize}
          listFontSize={settings.listFontSize}
          navFontSize={settings.navFontSize}
          showColSize={settings.listColSize}
          showColType={settings.listColType}
          showColTime={settings.listColTime}
          listNameWidth={settings.listNameWidth}
          onListNameWidth={props.setNameWidth}
          colSize={settings.listSizeWidth}
          colType={settings.listTypeWidth}
          colTime={settings.listTimeWidth}
          colDriveCapacity={settings.driveCapacityWidth}
          colDriveFree={settings.driveFreeWidth}
          onColWidths={props.setColWidths}
          dropHint={drag?.target?.paneId === pane.id ? drag.target.mode : null}
          draggingTabId={drag?.tabId ?? null}
          onFocusPane={() => props.focusPane(pane.id)}
          onNewTab={() => props.newTab(pane.id)}
          onOpenInNewTab={(path) => props.openTabAt(pane.id, path)}
          newTabOnMiddleClick={settings.newTabOnMiddleClick}
          onCloseTab={(tabId) => props.closeTab(pane.id, tabId)}
          onActivateTab={(tabId) => props.activateTab(pane.id, tabId)}
          onOpenTarget={props.openTarget}
          onNavigate={(tabId, path) => props.navigate(tabId, path)}
          onHistoryMove={props.historyMove}
          onTabDragStart={props.startTabDrag}
          onSortChange={(sort) => props.setPaneSort(pane.id, sort)}
          onToggleHidden={() => props.toggleHidden(pane.id)}
          onClipboard={(cut) => props.copySelection(pane.id, cut)}
          onCopyPath={props.copyCurrentPath}
          onPathInvalid={props.reportInvalidPath}
          onRowContextMenu={(entry, point) => props.openRowMenu(pane.id, entry, point)}
          onRowDragStart={(entry, ev) => props.startRowDrag(pane.id, entry, ev)}
          onBlankContextMenu={(point) => props.openBlankMenu(pane.id, point)}
          onTabContextMenu={(tabId, point) => props.openTabMenu(pane.id, tabId, point)}
          drives={driveList}
          onDriveMenu={(point) => props.openDriveMenu(pane.id, point)}
          onDriveRowMenu={props.openDriveRowMenu}
          onProperties={(names) => props.openProperties(pane.id, names)}
          onRename={(name) => props.startRename(pane.id, name)}
          onRenameMany={(names) => props.startRenameMany(pane.id, names)}
          onDelete={(names, permanent) => props.deleteEntries(pane.id, names, permanent)}
          onPaste={() => props.pasteInto(pane.id)}
          onRefresh={props.manualRefresh}
          onRetry={props.retryTab}
          keys={keys}
          update={props.update}
        />
      </div>
    )
  }

  return (
    <SplitView
      key={node.id}
      node={node}
      render={(childNode) => <PaneNodeRenderer {...props} node={childNode} />}
      onRatio={props.applyRatio}
    />
  )
}
