import { useCallback, useMemo } from 'react'
import type { MutableRefObject } from 'react'
import {
  archiveExtractItems,
  clipboardCopy,
  clipboardPaste,
  shellProperties,
} from '../api'
import type { ListData } from '../types'
import type { Translate } from '../i18n'
import type { Workspace } from '../workspace'
import { isInsideArchive, joinPath } from '../paths'

const errorCode = (err: unknown): string =>
  err && typeof err === 'object' && 'code' in err && typeof err.code === 'string'
    ? err.code
    : String(err)

export interface DeleteConfirmModalState {
  paneId: string
  names: string[]
  permanent: boolean
}

export interface PromptModalState {
  paneId: string
  title: string
  initial: string
  submit: (value: string) => void
}

export interface BatchRenameModalState {
  paneId: string
  dir: string
  names: string[]
}

export interface UseFileActionHandlersOptions {
  wsRef: MutableRefObject<Workspace | null>
  paneEls: MutableRefObject<Map<string, HTMLDivElement>>
  listData: Record<string, ListData>
  runOp: (
    kind: 'copy' | 'move' | 'delete' | 'rename' | 'newdir',
    sources: string[],
    options?: {
      targetDir?: string
      newName?: string
      newNames?: string[]
      permanent?: boolean
      after?: () => void
    },
  ) => void
  showToast: (text: string) => void
  t: Translate
  refreshPanes: () => void
  invalidateTree: (dir: string) => void
  invalidateGit: (dir: string) => void
  setPrompt: React.Dispatch<React.SetStateAction<PromptModalState | null>>
  setBatchRename: React.Dispatch<React.SetStateAction<BatchRenameModalState | null>>
  setDeleteConfirm: React.Dispatch<React.SetStateAction<DeleteConfirmModalState | null>>
  deleteConfirm: DeleteConfirmModalState | null
}

export function useFileActionHandlers({
  wsRef,
  paneEls,
  listData,
  runOp,
  showToast,
  t,
  refreshPanes,
  invalidateTree,
  invalidateGit,
  setPrompt,
  setBatchRename,
  setDeleteConfirm,
  deleteConfirm,
}: UseFileActionHandlersOptions) {
  const pathsFor = useCallback(
    (paneId: string, names: string[]): string[] => {
      const tabId = wsRef.current?.panes[paneId]?.activeTabId
      const tab = tabId ? wsRef.current?.tabs[tabId] : undefined
      if (!tab?.path) return []
      return names.map((name) => joinPath(tab.path, name))
    },
    [wsRef],
  )

  const focusList = useCallback(
    (paneId: string) => {
      window.setTimeout(() => {
        const el = paneEls.current.get(paneId)?.querySelector<HTMLElement>('[data-list]')
        el?.focus({ preventScroll: true })
      }, 0)
    },
    [paneEls],
  )

  const deleteEntries = useCallback((paneId: string, names: string[], permanent: boolean) => {
    if (!names.length) return
    setDeleteConfirm({ paneId, names, permanent })
  }, [setDeleteConfirm])

  const confirmDetail = useMemo(() => {
    if (!deleteConfirm) return ''
    const names = deleteConfirm.names
    const shown = names.slice(0, 3).join(', ')
    return names.length > 3 ? t('confirm.more', { names: shown, count: names.length }) : shown
  }, [deleteConfirm, t])

  const openProperties = useCallback(
    (paneId: string, names: string[]) => {
      for (const p of pathsFor(paneId, names)) {
        void shellProperties(p).catch((e: unknown) => showToast(t('op.failed', { code: errorCode(e) })))
      }
    },
    [pathsFor, showToast, t],
  )

  const startRename = useCallback(
    (paneId: string, name: string) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (!tab?.path) return
      setPrompt({
        paneId,
        title: t('dialog.renameTitle'),
        initial: name,
        submit: (value) => {
          if (!value || value === name) return
          runOp('rename', [joinPath(tab.path, name)], {
            targetDir: tab.path,
            newName: value,
          })
        },
      })
    },
    [runOp, setPrompt, t, wsRef],
  )

  const startRenameMany = useCallback(
    (paneId: string, names: string[]) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (!tab?.path) return
      if (names.length < 2) {
        if (names[0]) startRename(paneId, names[0])
        return
      }
      setBatchRename({ paneId, dir: tab.path, names })
    },
    [setBatchRename, startRename, wsRef],
  )

  const createFolder = useCallback(
    (paneId: string) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (!tab?.path) return
      setPrompt({
        paneId,
        title: t('dialog.newFolderTitle'),
        initial: '',
        submit: (value) => runOp('newdir', [tab.path], { targetDir: tab.path, newName: value }),
      })
    },
    [runOp, setPrompt, t, wsRef],
  )

  const pasteInto = useCallback(
    (paneId: string) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[paneId]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (!tab?.path) return
      void clipboardPaste()
        .then((clip) => {
          if (!clip.paths.length) {
            showToast(t('toast.noClipboard'))
            return
          }
          if (clip.paths.some(isInsideArchive) && !isInsideArchive(tab.path)) {
            void archiveExtractItems(clip.paths, tab.path)
              .then((extracted) => {
                invalidateTree(tab.path)
                invalidateGit(tab.path)
                refreshPanes()
                showToast(t('toast.copied', { count: extracted.length }))
              })
              .catch(() => showToast(t('toast.copyFailed')))
            return
          }
          runOp(clip.cut ? 'move' : 'copy', clip.paths, { targetDir: tab.path })
        })
        .catch(() => showToast(t('toast.noClipboard')))
    },
    [invalidateGit, invalidateTree, refreshPanes, runOp, showToast, t, wsRef],
  )

  const copyPaths = useCallback(
    (paths: string[]) => {
      const text = paths.join('\r\n')
      void navigator.clipboard
        .writeText(text)
        .then(() => showToast(t('toast.copied', { count: paths.length })))
        .catch(() => showToast(t('toast.copyFailed')))
    },
    [showToast, t],
  )

  const copyCurrentPath = useCallback(
    (path: string) => {
      void navigator.clipboard
        .writeText(path)
        .then(() => showToast(t('toast.pathCopied')))
        .catch(() => showToast(t('toast.copyFailed')))
    },
    [showToast, t],
  )

  const reportInvalidPath = useCallback(
    (path: string) => showToast(t('toast.pathInvalid', { path })),
    [showToast, t],
  )

  const copySelection = useCallback(
    (paneId: string, cut: boolean) => {
      const tabId = wsRef.current?.panes[paneId]?.activeTabId
      const tab = tabId ? wsRef.current?.tabs[tabId] : undefined
      if (!tab) return
      const list = listData[tab.id]
      const names = list ? Array.from(list.selected) : []
      if (!names.length) return
      const paths = names.map((name) => joinPath(tab.path, name))
      clipboardCopy(paths, cut)
        .then(() => showToast(cut ? t('toast.cut', { count: paths.length }) : t('toast.copied', { count: paths.length })))
        .catch(() => showToast(t('toast.copyFailed')))
    },
    [listData, showToast, t, wsRef],
  )

  return {
    pathsFor,
    focusList,
    deleteEntries,
    confirmDetail,
    openProperties,
    startRename,
    startRenameMany,
    createFolder,
    pasteInto,
    copyPaths,
    copyCurrentPath,
    reportInvalidPath,
    copySelection,
  }
}
