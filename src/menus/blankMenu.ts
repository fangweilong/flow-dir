import type { MutableRefObject } from 'react'
import type { MenuItem } from '../ContextMenu'
import type { MenuPoint } from '../PaneView'
import type { ListData } from '../types'
import type { Translate } from '../i18n'
import type { ShortcutId } from '../shortcuts'
import type { Workspace } from '../workspace'
import { filterMatches } from '../types'
import { explorerOpen, openTerminal } from '../api'
import { errorCode } from '../errorCode'
import { menuHint } from '../shortcuts'
import { isInsideArchive, isShellPath, isWebdavPath } from '../paths'
import type { FileToolsTarget } from './rowMenu'

export type BlankMenuOptions = {
  wsRef: MutableRefObject<Workspace | null>
  t: Translate
  keys: Record<ShortcutId, string>
  update: (tabId: string, fn: (d: ListData) => ListData) => void
  manualRefresh: () => void
  toggleHidden: (paneId: string) => void
  createFolder: (paneId: string) => void
  pasteInto: (paneId: string) => void
  startFolderSize: (tabId: string, path: string) => void
  setFileTools: (val: FileToolsTarget) => void
  setCompareOpen: (open: boolean) => void
  showToast: (msg: string) => void
}

/**
 * Menu for rows the shell cannot host (archive listings and WebDAV), and for a pane with
 * no path yet. Local folders go through `shellMenuBackgroundShow` for shell extensions.
 */
export function buildBlankMenu(
  options: BlankMenuOptions,
  paneId: string,
  point: MenuPoint,
): MenuItem[] | null {
  const { wsRef, t, keys } = options
  const current = wsRef.current
  if (!current) return null
  const pane = current.panes[paneId]
  const tab = pane ? current.tabs[pane.activeTabId] : undefined
  if (!tab) return null
  const inArchive = Boolean(tab.path && isInsideArchive(tab.path))
  // Shell locations expose only the shell's own commands; local file operations and
  // scans have no filesystem path to run against.
  const inShell = Boolean(tab.path && isShellPath(tab.path))
  // WebDAV still supports new folder/paste/refresh, but terminal, Explorer, folder
  // size, duplicate scan and compare all need a local filesystem path.
  const remote = Boolean(tab.path && isWebdavPath(tab.path))
  return tab.path
    ? [
        ...(!inArchive && !inShell
          ? [
              {
                kind: 'item' as const,
                label: t('menu.newFolder'),
                onSelect: () => options.createFolder(paneId),
              },
              {
                kind: 'item' as const,
                label: t('menu.paste'),
                hint: menuHint(keys.paste),
                onSelect: () => options.pasteInto(paneId),
              },
              { kind: 'sep' as const },
            ]
          : []),
        {
          kind: 'item',
          label: t('menu.refresh'),
          hint: menuHint(keys.refresh),
          onSelect: () => options.manualRefresh(),
        },
        {
          kind: 'item',
          label: t('menu.selectAll'),
          hint: menuHint(keys.selectAll),
          onSelect: () => {
            options.update(tab.id, (d) => {
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
        ...(!inArchive && !inShell && !remote
          ? [
              {
                kind: 'item' as const,
                label: t('menu.openTerminal'),
                onSelect: () =>
                  void openTerminal(tab.path).catch((e: unknown) =>
                    options.showToast(t('op.failed', { code: errorCode(e) })),
                  ),
              },
              {
                kind: 'item' as const,
                label: t('menu.openInExplorer'),
                onSelect: () =>
                  void explorerOpen(tab.path).catch((e: unknown) =>
                    options.showToast(t('op.failed', { code: errorCode(e) })),
                  ),
              },
            ]
          : []),
        {
          kind: 'item',
          label: pane.hidden ? t('list.hideHidden') : t('list.showHidden'),
          onSelect: () => options.toggleHidden(paneId),
        },
        ...(!inShell && !remote
          ? [
              {
                kind: 'item' as const,
                label: t('menu.folderSize'),
                onSelect: () => options.startFolderSize(tab.id, tab.path),
              },
              {
                kind: 'item' as const,
                label: t('menu.findDuplicates'),
                onSelect: () => options.setFileTools({ mode: 'dup', root: tab.path }),
              },
              {
                kind: 'item' as const,
                label: t('compare.entry'),
                onSelect: () => options.setCompareOpen(true),
              },
            ]
          : []),
      ]
    : [
        {
          kind: 'item',
          label: t('menu.refresh'),
          hint: menuHint(keys.refresh),
          onSelect: () => options.manualRefresh(),
        },
      ]
}
