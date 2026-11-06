import type { MutableRefObject } from 'react'
import type { MenuItem } from '../ContextMenu'
import type { Entry } from '../types'
import type { Translate } from '../i18n'
import type { ShortcutId } from '../shortcuts'
import type { Workspace } from '../workspace'
import type { CompressFormat } from '../api'
import { clipboardCopy, openTerminal } from '../api'
import { errorCode } from '../errorCode'
import { menuHint } from '../shortcuts'
import {
  baseName,
  entryPath,
  isArchiveName,
  isDir,
  isInsideArchive,
  isWebdavPath,
} from '../paths'

export type FileToolsTarget =
  | { mode: 'hash'; paths: string[] }
  | { mode: 'dup'; root: string }
  | null

export type RowMenuOptions = {
  wsRef: MutableRefObject<Workspace | null>
  t: Translate
  keys: Record<ShortcutId, string>
  navigate: (tabId: string, path: string) => void
  openTabAt?: (paneId: string, path: string) => void
  openTarget: (path: string) => void
  showToast: (msg: string) => void
  copyPaths: (paths: string[]) => void
  startRename: (paneId: string, name: string) => void
  startRenameMany: (paneId: string, names: string[]) => void
  deleteEntries: (paneId: string, names: string[], permanent: boolean) => void
  pasteInto: (paneId: string) => void
  extractArchive: (archivePath: string, destDir: string) => void
  setExtractTarget: (val: { archive: string; initial: string } | null) => void
  requestCompress: (paths: string[], dir: string, format: CompressFormat) => void
  setFileTools: (val: FileToolsTarget) => void
  isPinnedPlace: (path: string) => boolean
  togglePinPlace: (path: string) => void
}

/**
 * Self-drawn menu for rows the native shell menu cannot serve: archive members and
 * WebDAV folders. Local rows go through `shellMenuShow` instead so shell extensions
 * (7-Zip, TortoiseGit, …) are available.
 */
export function buildRowMenu(
  options: RowMenuOptions,
  paneId: string,
  entry: Entry,
  paths: string[],
): MenuItem[] {
  const { wsRef, t, keys, navigate, openTabAt, openTarget, showToast, copyPaths } = options
  const dir = isDir(entry.attrs)
  const pane = wsRef.current?.panes[paneId]
  const tabPath = pane ? wsRef.current?.tabs[pane.activeTabId]?.path : undefined
  const inArchive = Boolean(tabPath && isInsideArchive(tabPath))
  // Remote archives cannot be extracted in-process, and the shell menu does not run
  // for a WebDAV folder at all, so the fallback menu is the only menu they get.
  const remote = Boolean(tabPath && isWebdavPath(tabPath))
  const isArchive =
    !remote && !dir && paths.length === 1 && tabPath && !inArchive && isArchiveName(entry.name)
  const archivePath = isArchive ? entryPath(tabPath!, entry) : null
  const archiveMenu: MenuItem[] = archivePath
    ? [
        { kind: 'sep' },
        {
          kind: 'item',
          label: t('menu.extractHere'),
          onSelect: () => options.extractArchive(archivePath, tabPath!),
        },
        {
          kind: 'item',
          label: t('menu.extractTo'),
          onSelect: () => options.setExtractTarget({ archive: archivePath, initial: tabPath! }),
        },
      ]
    : []
  const compressMenu: MenuItem[] =
    tabPath !== undefined && !remote
      ? [
          {
            kind: 'item',
            label: t('menu.compressTo'),
            onSelect: () => options.requestCompress(paths, tabPath, 'zip'),
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
        const livePane = current?.panes[paneId]
        const tab = livePane ? current?.tabs[livePane.activeTabId] : undefined
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
          {
            kind: 'item' as const,
            label: t('menu.paste'),
            hint: menuHint(keys.paste),
            onSelect: () => options.pasteInto(paneId),
          },
          { kind: 'sep' as const },
          {
            kind: 'item' as const,
            label: paths.length > 1 ? t('menu.batchRename') : t('menu.rename'),
            hint: menuHint(keys.rename),
            onSelect: () =>
              paths.length > 1
                ? options.startRenameMany(paneId, paths.map(baseName))
                : options.startRename(paneId, entry.name),
          },
          {
            kind: 'item' as const,
            label: t('menu.delete'),
            hint: menuHint(keys.delete),
            danger: true,
            onSelect: () => options.deleteEntries(paneId, [entry.name], false),
          },
          ...(remote
            ? []
            : [
                {
                  kind: 'item' as const,
                  label: t('menu.deletePermanent'),
                  hint: menuHint(`shift+${keys.delete}`),
                  danger: true,
                  onSelect: () => options.deleteEntries(paneId, [entry.name], true),
                },
              ]),
          ...archiveMenu,
          ...compressMenu,
          ...(dir && paths.length === 1
            ? [
                { kind: 'sep' as const },
                {
                  kind: 'item' as const,
                  label: options.isPinnedPlace(paths[0])
                    ? t('sidebar.unpinPlace')
                    : t('sidebar.pinPlace'),
                  onSelect: () => options.togglePinPlace(paths[0]),
                },
                ...(remote
                  ? []
                  : [
                      {
                        kind: 'item' as const,
                        label: t('menu.openTerminal'),
                        onSelect: () =>
                          void openTerminal(paths[0]).catch((e: unknown) =>
                            showToast(t('op.failed', { code: errorCode(e) })),
                          ),
                      },
                    ]),
              ]
            : []),
        ]
      : []),
    ...(remote
      ? []
      : [
          { kind: 'sep' as const },
          {
            kind: 'item' as const,
            label: t('menu.hash'),
            onSelect: () => options.setFileTools({ mode: 'hash', paths }),
          },
        ]),
    { kind: 'sep' },
    { kind: 'item', label: t('menu.copyPath'), onSelect: () => copyPaths(paths) },
  ]
}
