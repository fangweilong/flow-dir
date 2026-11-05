import { useCallback, useEffect, useState } from 'react'
import type { MutableRefObject } from 'react'
import type { MenuItem } from '../ContextMenu'
import type { MenuPoint } from '../PaneView'
import type { Drive, Entry, Lang, ListData } from '../types'
import { filterMatches } from '../types'
import { samePath } from '../workspace'
import type { Workspace } from '../workspace'
import type { ShortcutId } from '../shortcuts'
import type { Translate } from '../i18n'
import { driveKindLabel } from '../i18n'
import { menuHint } from '../shortcuts'
import { baseName, entryPath, isArchiveName, isDir, isFsPath, isInsideArchive, isShellPath, joinPath } from '../paths'
import {
  clipboardCopy,
  COMPRESS_FORMATS,
  driveEject,
  explorerOpen,
  openTerminal,
  shellMenuBackgroundShow,
  shellMenuShow,
  shellProperties,
} from '../api'
import type { CompressFormat } from '../api'

export type QuickPlaceEntry = { key: string; label: string; path: string; custom: boolean }

const errorCode = (err: unknown): string =>
  err && typeof err === 'object' && 'code' in err && typeof err.code === 'string'
    ? err.code
    : String(err)

export interface UsePaneContextMenusOptions {
  wsRef: MutableRefObject<Workspace | null>
  listData: Record<string, ListData>
  driveList: Drive[]
  keys: Record<ShortcutId, string>
  t: Translate
  lang: Lang
  showToast: (msg: string) => void
  setMenu: (menu: { x: number; y: number; items: MenuItem[] } | null) => void
  navigate: (tabId: string, path: string) => void
  openTabAt?: (paneId: string, path: string) => void
  openTarget: (path: string) => void
  update: (tabId: string, fn: (d: ListData) => ListData) => void
  refreshPanes: () => void
  manualRefresh: () => void
  refreshDrives: () => void
  toggleHidden: (paneId: string) => void
  createFolder: (paneId: string) => void
  pasteInto: (paneId: string) => void
  copyPaths: (paths: string[]) => void
  startRename: (paneId: string, name: string) => void
  startRenameMany: (paneId: string, names: string[]) => void
  deleteEntries: (paneId: string, names: string[], permanent: boolean) => void
  extractArchive: (archivePath: string, destDir: string) => void
  setExtractTarget: (val: { archive: string; initial: string } | null) => void
  requestCompress: (paths: string[], dir: string, format: CompressFormat) => void
  setFileTools: (val: { mode: 'hash'; paths: string[] } | { mode: 'dup'; root: string } | null) => void
  setCompareOpen: (open: boolean) => void
  startFolderSize: (tabId: string, path: string) => void
  closeTab: (paneId: string, tabId: string) => void
  closeOthers: (paneId: string, tabId: string) => void
  closeRight: (paneId: string, tabId: string) => void
  togglePin: (paneId: string, tabId: string) => void
  duplicateTab: (paneId: string, tabId: string) => void
  openInNewPane: (paneId: string, tabId: string) => void
  reopenTab: () => void
  isPinnedPlace: (path: string) => boolean
  togglePinPlace: (path: string) => void
  removePlace: (place: QuickPlaceEntry) => void
}

export function usePaneContextMenus({
  wsRef,
  listData,
  driveList,
  keys,
  t,
  showToast,
  setMenu,
  navigate,
  openTabAt,
  openTarget,
  update,
  refreshPanes,
  manualRefresh,
  refreshDrives,
  toggleHidden,
  createFolder,
  pasteInto,
  copyPaths,
  startRename,
  startRenameMany,
  deleteEntries,
  extractArchive,
  setExtractTarget,
  requestCompress,
  setFileTools,
  setCompareOpen,
  startFolderSize,
  closeTab,
  closeOthers,
  closeRight,
  togglePin,
  duplicateTab,
  openInNewPane,
  reopenTab,
  isPinnedPlace,
  togglePinPlace,
  removePlace,
}: UsePaneContextMenusOptions) {
  /** A shell "New" command just ran; wait for the rescan that reveals the created item. */
  const [pendingNew, setPendingNew] = useState<{
    paneId: string
    tabId: string
    /** Folder the shell menu was opened in; navigating away invalidates the marker. */
    path: string
    before: Set<string>
  } | null>(null)

  useEffect(() => {
    if (!pendingNew) return
    // A cancelled or failed shell "New" never adds an entry; without this the marker
    // would hijack the next unrelated new file that appears in the folder.
    const timer = window.setTimeout(() => setPendingNew(null), 5_000)
    return () => window.clearTimeout(timer)
  }, [pendingNew])

  useEffect(() => {
    if (!pendingNew) return
    const live = wsRef.current?.tabs[pendingNew.tabId]
    // The tab may be closed or sent elsewhere while the shell dialog is open; a rescan of
    // a different folder must not be read as the created item.
    if (!live || !samePath(live.path, pendingNew.path)) return
    const listing = listData[pendingNew.tabId]
    // Wait for the finished scan: a partial listing cannot tell a new entry from one the
    // enumeration has simply not reached yet.
    if (!listing || listing.loading) return
    const entries = listing.entries
    const created = entries.filter((entry) => !pendingNew.before.has(entry.name))
    if (!created.length) return
    setPendingNew(null)
    // Another process (build, download, sync) can add a file in the same window. Renaming
    // the wrong one is worse than skipping the convenience prompt.
    if (created.length > 1) return
    const [entry] = created
    update(pendingNew.tabId, (data) => ({
      ...data,
      selected: new Set([entry.name]),
      anchor: entry.name,
    }))
    startRename(pendingNew.paneId, entry.name)
  }, [listData, pendingNew, startRename, update, wsRef])

  const rowFallback = useCallback(
    (paneId: string, entry: Entry, paths: string[]): MenuItem[] => {
      const dir = isDir(entry.attrs)
      const pane = wsRef.current?.panes[paneId]
      const tabPath = pane ? wsRef.current?.tabs[pane.activeTabId]?.path : undefined
      const inArchive = Boolean(tabPath && isInsideArchive(tabPath))
      const isArchive = !dir && paths.length === 1 && tabPath && !inArchive && isArchiveName(entry.name)
      const archivePath = isArchive ? entryPath(tabPath!, entry) : null
      const archiveMenu: MenuItem[] = archivePath
        ? [
            { kind: 'sep' },
            {
              kind: 'item',
              label: t('menu.extractHere'),
              onSelect: () => extractArchive(archivePath, tabPath!),
            },
            {
              kind: 'item',
              label: t('menu.extractTo'),
              onSelect: () => setExtractTarget({ archive: archivePath, initial: tabPath! }),
            },
          ]
        : []
      const compressMenu: MenuItem[] =
        tabPath !== undefined
          ? [
              {
                kind: 'item',
                label: t('menu.compressTo'),
                onSelect: () => requestCompress(paths, tabPath, 'zip'),
              },
            ]
          : []
      return [
        {
          kind: 'item',
          label: t('menu.open'),
          hint: menuHint(keys.open),
          onSelect: () => {
            const current = wsRef.current
            const pane = current?.panes[paneId]
            const tab = pane ? current?.tabs[pane.activeTabId] : undefined
            if (!tab) return
            const target = entryPath(tab.path, entry)
            if (dir || isArchive) navigate(tab.id, target)
            else openTarget(target)
          },
        },
        ...(dir || isArchive
          ? [
              {
                kind: 'item' as const,
                label: t('menu.openInNewTab'),
                onSelect: () => {
                  if (tabPath) {
                    const target = entryPath(tabPath, entry)
                    if (openTabAt) openTabAt(paneId, target)
                    else if (pane) navigate(pane.activeTabId, target)
                  }
                },
              },
            ]
          : []),
        {
          kind: 'item',
          label: t('menu.copy'),
          hint: menuHint(keys.copy),
          onSelect: () => {
            clipboardCopy(paths, false)
              .then(() => showToast(t('toast.copied', { count: paths.length })))
              .catch(() => showToast(t('toast.copyFailed')))
          },
        },
        ...(!inArchive
          ? [
              {
                kind: 'item' as const,
                label: t('menu.cut'),
                hint: menuHint(keys.cut),
                onSelect: () => {
                  clipboardCopy(paths, true)
                    .then(() => showToast(t('toast.cut', { count: paths.length })))
                    .catch(() => showToast(t('toast.copyFailed')))
                },
              },
              { kind: 'item' as const, label: t('menu.paste'), hint: menuHint(keys.paste), onSelect: () => pasteInto(paneId) },
              { kind: 'sep' as const },
              {
                kind: 'item' as const,
                label: paths.length > 1 ? t('menu.batchRename') : t('menu.rename'),
                hint: menuHint(keys.rename),
                onSelect: () =>
                  paths.length > 1
                    ? startRenameMany(paneId, paths.map(baseName))
                    : startRename(paneId, entry.name),
              },
              {
                kind: 'item' as const,
                label: t('menu.delete'),
                hint: menuHint(keys.delete),
                danger: true,
                onSelect: () => deleteEntries(paneId, [entry.name], false),
              },
              {
                kind: 'item' as const,
                label: t('menu.deletePermanent'),
                hint: menuHint(`shift+${keys.delete}`),
                danger: true,
                onSelect: () => deleteEntries(paneId, [entry.name], true),
              },
              ...archiveMenu,
              ...compressMenu,
              ...(dir && paths.length === 1
                ? [
                    { kind: 'sep' as const },
                    {
                      kind: 'item' as const,
                      label: isPinnedPlace(paths[0]) ? t('sidebar.unpinPlace') : t('sidebar.pinPlace'),
                      onSelect: () => togglePinPlace(paths[0]),
                    },
                    {
                      kind: 'item' as const,
                      label: t('menu.openTerminal'),
                      onSelect: () =>
                        void openTerminal(paths[0]).catch((e: unknown) =>
                          showToast(t('op.failed', { code: errorCode(e) })),
                        ),
                    },
                  ]
                : []),
            ]
          : []),
        { kind: 'sep' },
        { kind: 'item', label: t('menu.hash'), onSelect: () => setFileTools({ mode: 'hash', paths }) },
        { kind: 'item', label: t('menu.copyPath'), onSelect: () => copyPaths(paths) },
      ]
    },
    [
      copyPaths,
      deleteEntries,
      extractArchive,
      isPinnedPlace,
      keys,
      navigate,
      openTabAt,
      openTarget,
      requestCompress,
      setExtractTarget,
      setFileTools,
      showToast,
      startRename,
      startRenameMany,
      t,
      togglePinPlace,
      wsRef,
    ],
  )

  const openRowMenu = useCallback(
    (paneId: string, entry: Entry, point: MenuPoint) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (!tab) return
      const selected = listData[tab.id]?.selected
      const names = selected && selected.has(entry.name) ? Array.from(selected) : [entry.name]
      // Rows from a shell folder carry their own parsing names; joining the folder path
      // would produce a name the shell cannot resolve.
      const listed = listData[tab.id]?.entries
      const byName = listed ? new Map(listed.map((item) => [item.name, item])) : null
      const paths = names.map((name) => {
        const item = byName?.get(name)
        return item ? entryPath(tab.path, item) : joinPath(tab.path, name)
      })
      if (tab.path && isInsideArchive(tab.path)) {
        setMenu({ x: point.clientX, y: point.clientY, items: rowFallback(paneId, entry, paths) })
        return
      }
      const dpr = window.devicePixelRatio || 1
      void shellMenuShow(
        paths,
        Math.round(point.screenX * dpr),
        Math.round(point.screenY * dpr),
        point.shiftKey,
        t('menu.hash'),
        { title: t('menu.extract'), here: t('menu.extractHere'), to: t('menu.extractTo') },
        {
          title: t('menu.compress'),
          formats: COMPRESS_FORMATS.map((format) => ({
            id: format,
            label: format.toUpperCase(),
          })),
        },
      )
        .then((outcome) => {
          if (outcome.action === 'hash') setFileTools({ mode: 'hash', paths })
          else if (outcome.action === 'extractHere') extractArchive(paths[0], tab.path)
          else if (outcome.action === 'extractTo') {
            setExtractTarget({ archive: paths[0], initial: tab.path })
          } else if (outcome.action === 'compress' && outcome.format) {
            requestCompress(paths, tab.path, outcome.format as CompressFormat)
          } else if (outcome.invoked) refreshPanes()
        })
        .catch(() => {
          setMenu({ x: point.clientX, y: point.clientY, items: rowFallback(paneId, entry, paths) })
        })
    },
    [
      extractArchive,
      listData,
      refreshPanes,
      requestCompress,
      rowFallback,
      setExtractTarget,
      setFileTools,
      setMenu,
      t,
      wsRef,
    ],
  )

  const blankFallbackMenu = useCallback(
    (paneId: string, point: MenuPoint) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (!tab) return
      const inArchive = Boolean(tab.path && isInsideArchive(tab.path))
      // Shell locations expose only the shell's own commands; local file operations and
      // scans have no filesystem path to run against.
      const inShell = Boolean(tab.path && isShellPath(tab.path))
      const items: MenuItem[] = tab.path
        ? [
            ...(!inArchive && !inShell
              ? [
                  { kind: 'item' as const, label: t('menu.newFolder'), onSelect: () => createFolder(paneId) },
                  { kind: 'item' as const, label: t('menu.paste'), hint: menuHint(keys.paste), onSelect: () => pasteInto(paneId) },
                  { kind: 'sep' as const },
                ]
              : []),
            { kind: 'item', label: t('menu.refresh'), hint: menuHint(keys.refresh), onSelect: () => manualRefresh() },
            {
              kind: 'item',
              label: t('menu.selectAll'),
              hint: menuHint(keys.selectAll),
              onSelect: () => {
                update(tab.id, (d) => {
                  // Select what the user can see: the pane invariant is that filtered-out
                  // rows never enter the selection (copy/drag read it directly).
                  const all = d.entries
                    .filter((entry) => filterMatches(entry.name, d.filter))
                    .map((entry) => entry.name)
                  return { ...d, selected: new Set(all), anchor: all[0] ?? '' }
                })
              },
            },
            { kind: 'sep' },
            ...(!inArchive && !inShell
              ? [
                  {
                    kind: 'item' as const,
                    label: t('menu.openTerminal'),
                    onSelect: () =>
                      void openTerminal(tab.path).catch((e: unknown) =>
                        showToast(t('op.failed', { code: errorCode(e) })),
                      ),
                  },
                  {
                    kind: 'item' as const,
                    label: t('menu.openInExplorer'),
                    onSelect: () =>
                      void explorerOpen(tab.path).catch((e: unknown) =>
                        showToast(t('op.failed', { code: errorCode(e) })),
                      ),
                  },
                ]
              : []),
            {
              kind: 'item',
              label: pane.hidden ? t('list.hideHidden') : t('list.showHidden'),
              onSelect: () => toggleHidden(paneId),
            },
            ...(!inShell
              ? [
                  {
                    kind: 'item' as const,
                    label: t('menu.folderSize'),
                    onSelect: () => startFolderSize(tab.id, tab.path),
                  },
                  {
                    kind: 'item' as const,
                    label: t('menu.findDuplicates'),
                    onSelect: () => setFileTools({ mode: 'dup', root: tab.path }),
                  },
                  {
                    kind: 'item' as const,
                    label: t('compare.entry'),
                    onSelect: () => setCompareOpen(true),
                  },
                ]
              : []),
          ]
        : [{ kind: 'item', label: t('menu.refresh'), hint: menuHint(keys.refresh), onSelect: () => manualRefresh() }]

      setMenu({ x: point.clientX, y: point.clientY, items })
    },
    [
      createFolder,
      keys,
      listData,
      manualRefresh,
      pasteInto,
      setCompareOpen,
      setFileTools,
      setMenu,
      showToast,
      t,
      toggleHidden,
      update,
      wsRef,
    ],
  )

  const openBlankMenu = useCallback(
    (paneId: string, point: MenuPoint) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (!pane || !tab) return
      if (!tab.path) {
        blankFallbackMenu(paneId, point)
        return
      }
      // Captured once: the shell dialog is modal and this folder is what the marker means.
      const dir = tab.path
      const dpr = window.devicePixelRatio || 1
      const before = new Set((listData[tab.id]?.entries ?? []).map((entry) => entry.name))
      void shellMenuBackgroundShow(
        dir,
        Math.round(point.screenX * dpr),
        Math.round(point.screenY * dpr),
        point.shiftKey,
        pane.hidden ? t('list.hideHidden') : t('list.showHidden'),
        t('menu.findDuplicates'),
        t('compare.entry'),
      )
        .then((outcome) => {
          if (outcome.action === 'toggleHidden') toggleHidden(paneId)
          else if (outcome.action === 'findDuplicates') setFileTools({ mode: 'dup', root: dir })
          else if (outcome.action === 'compare') setCompareOpen(true)
          else if (outcome.action === 'newItem') {
            setPendingNew({ paneId, tabId: tab.id, path: dir, before })
            refreshPanes()
          } else if (outcome.invoked) refreshPanes()
        })
        .catch(() => blankFallbackMenu(paneId, point))
    },
    [blankFallbackMenu, listData, refreshPanes, setCompareOpen, setFileTools, startFolderSize, t, toggleHidden, wsRef],
  )

  const openTabMenu = useCallback(
    (paneId: string, tabId: string, point: MenuPoint) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = current.tabs[tabId]
      if (!pane || !tab) return
      const count = pane.tabIds.length
      const index = pane.tabIds.indexOf(tabId)
      const hasRight = index >= 0 && index < count - 1
      const isPinned = Boolean(tab.pinned)
      const otherUnpinned = pane.tabIds.some((id) => id !== tabId && !current.tabs[id]?.pinned)

      const items: MenuItem[] = [
        {
          kind: 'item',
          label: isPinned ? t('menu.unpinTab') : t('menu.pinTab'),
          onSelect: () => togglePin(paneId, tabId),
        },
        {
          kind: 'item',
          label: t('menu.duplicateTab'),
          onSelect: () => duplicateTab(paneId, tabId),
        },
        { kind: 'sep' },
        {
          kind: 'item',
          label: t('menu.closeTab'),
          hint: menuHint(keys.closeTab),
          onSelect: () => closeTab(paneId, tabId),
        },
        {
          kind: 'item',
          label: t('menu.closeOthers'),
          disabled: !otherUnpinned,
          onSelect: () => closeOthers(paneId, tabId),
        },
        {
          kind: 'item',
          label: t('menu.closeRight'),
          disabled: !hasRight,
          onSelect: () => closeRight(paneId, tabId),
        },
        {
          kind: 'item',
          label: t('menu.reopenTab'),
          hint: menuHint(keys.reopenTab),
          onSelect: reopenTab,
        },
        { kind: 'sep' },
        { kind: 'item', label: t('menu.copyPath'), disabled: !tab.path, onSelect: () => copyPaths([tab.path]) },
        { kind: 'item', label: t('menu.openInNewPane'), onSelect: () => openInNewPane(paneId, tabId) },
      ]
      setMenu({ x: point.clientX, y: point.clientY, items })
    },
    [
      closeOthers,
      closeRight,
      closeTab,
      copyPaths,
      duplicateTab,
      keys,
      openInNewPane,
      reopenTab,
      setMenu,
      t,
      togglePin,
      wsRef,
    ],
  )

  const openPlaceMenu = useCallback(
    (place: QuickPlaceEntry, point: MenuPoint) => {
      const current = wsRef.current
      const pane = current?.panes[current.activePaneId]
      const tab = pane && current ? current.tabs[pane.activeTabId] : undefined
      const items: MenuItem[] = [
        {
          kind: 'item',
          label: t('menu.open'),
          disabled: !tab,
          onSelect: () => {
            if (tab) navigate(tab.id, place.path)
          },
        },
        {
          kind: 'item',
          label: t('menu.openInNewTab'),
          disabled: !pane,
          onSelect: () => {
            if (!pane) return
            if (openTabAt) openTabAt(pane.id, place.path)
            else navigate(pane.activeTabId, place.path)
          },
        },
        { kind: 'sep' },
        { kind: 'item', label: t('menu.copyPath'), onSelect: () => copyPaths([place.path]) },
        { kind: 'sep' },
        { kind: 'item', label: t('sidebar.removePlace'), danger: true, onSelect: () => removePlace(place) },
      ]
      setMenu({ x: point.clientX, y: point.clientY, items })
    },
    [copyPaths, navigate, openTabAt, removePlace, setMenu, t, wsRef],
  )

  const openTreeContextMenu = useCallback(
    (targetPath: string, point: { clientX: number; clientY: number }) => {
      const current = wsRef.current
      const pane = current?.panes[current.activePaneId]
      const tab = pane && current ? current.tabs[pane.activeTabId] : undefined
      const items: MenuItem[] = [
        {
          kind: 'item',
          label: t('menu.open'),
          disabled: !tab,
          onSelect: () => {
            if (tab) navigate(tab.id, targetPath)
          },
        },
        {
          kind: 'item',
          label: t('menu.openInNewTab'),
          disabled: !pane,
          onSelect: () => {
            if (!pane) return
            if (openTabAt) openTabAt(pane.id, targetPath)
            else navigate(pane.activeTabId, targetPath)
          },
        },
        {
          kind: 'item',
          label: t('menu.openInExplorer'),
          onSelect: () =>
            void explorerOpen(targetPath).catch((e: unknown) =>
              showToast(t('op.failed', { code: errorCode(e) })),
            ),
        },
        {
          kind: 'item',
          label: isPinnedPlace(targetPath) ? t('sidebar.unpinPlace') : t('sidebar.pinPlace'),
          onSelect: () => togglePinPlace(targetPath),
        },
        { kind: 'sep' },
        {
          kind: 'item',
          label: t('menu.copyPath'),
          onSelect: () => copyPaths([targetPath]),
        },
        {
          kind: 'item',
          label: t('menu.properties'),
          onSelect: () =>
            void shellProperties(targetPath).catch((e: unknown) =>
              showToast(t('op.failed', { code: errorCode(e) })),
            ),
        },
      ]
      setMenu({ x: point.clientX, y: point.clientY, items })
    },
    [copyPaths, isPinnedPlace, navigate, openTabAt, setMenu, showToast, t, togglePinPlace, wsRef],
  )

  const openDriveRowMenu = useCallback(
    (drive: Drive, point: MenuPoint) => {
      const current = wsRef.current
      const pane = current?.panes[current.activePaneId]
      const tab = pane && current ? current.tabs[pane.activeTabId] : undefined
      const items: MenuItem[] = [
        {
          kind: 'item',
          label: t('menu.open'),
          disabled: !tab,
          onSelect: () => {
            if (!tab) return
            if (isFsPath(drive.root)) navigate(tab.id, drive.root)
            else openTarget(drive.root)
          },
        },
        ...(isFsPath(drive.root)
          ? [
              {
                kind: 'item' as const,
                label: t('menu.openInNewTab'),
                disabled: !pane,
                onSelect: () => {
                  if (pane) {
                    if (openTabAt) openTabAt(pane.id, drive.root)
                    else navigate(pane.activeTabId, drive.root)
                  }
                },
              },
              {
                kind: 'item' as const,
                label: t('menu.openInExplorer'),
                onSelect: () =>
                  void explorerOpen(drive.root).catch((e: unknown) =>
                    showToast(t('op.failed', { code: errorCode(e) })),
                  ),
              },
            ]
          : []),
        {
          kind: 'item',
          label: t('menu.properties'),
          onSelect: () =>
            void shellProperties(drive.root).catch((e: unknown) =>
              showToast(t('op.failed', { code: errorCode(e) })),
            ),
        },
      ]
      if (drive.kind === 'removable' || drive.kind === 'cdrom') {
        items.push(
          { kind: 'sep' },
          {
            kind: 'item',
            label: t('menu.eject'),
            onSelect: () => {
              void driveEject(drive.root)
                .then(() => window.setTimeout(refreshDrives, 1200))
                .catch((e: unknown) => showToast(t('op.failed', { code: errorCode(e) })))
            },
          },
        )
      }
      setMenu({ x: point.clientX, y: point.clientY, items })
    },
    [navigate, openTabAt, openTarget, refreshDrives, setMenu, showToast, t, wsRef],
  )

  const openDriveMenu = useCallback(
    (paneId: string, point: MenuPoint) => {
      const current = wsRef.current
      const pane = current ? current.panes[paneId] : undefined
      const tab = pane && current ? current.tabs[pane.activeTabId] : undefined
      if (!tab) return
      const items: MenuItem[] = driveList.map((drive) => ({
        kind: 'item' as const,
        label: drive.name,
        hint: driveKindLabel(t, drive.kind),
        onSelect: () =>
          isFsPath(drive.root)
            ? navigate(tab.id, drive.root)
            : openTarget(drive.root),
      }))
      setMenu({ x: point.clientX, y: point.clientY, items })
    },
    [driveList, navigate, openTarget, setMenu, t, wsRef],
  )

  return {
    openRowMenu,
    rowFallback,
    blankFallbackMenu,
    openBlankMenu,
    openTabMenu,
    openPlaceMenu,
    openTreeContextMenu,
    openDriveRowMenu,
    openDriveMenu,
  }
}
