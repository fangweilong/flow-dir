import { useCallback, useEffect, useState } from 'react'
import type { PointerEvent as ReactPointerEvent, RefObject } from 'react'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import type { Entry, ListData, OpKind } from '../types'
import type { Workspace } from '../workspace'
import type { Translate } from '../i18n'
import type { RunOpOptions } from './useFileOperations'
import {
  archiveExtractItems,
  archivePrepareDrag,
  dragOut,
} from '../api'
import {
  isFsPath,
  isInsideArchive,
  joinPath,
  parentPath,
  pathKey,
  sameVolume,
  underPath,
} from '../paths'

export type DropTarget = { dir: string; left: number; top: number; width: number; height: number }

export type RowDragState = {
  names: string[]
  sourceDir: string
  x: number
  y: number
  started: boolean
  target: DropTarget | null
  copy: boolean
  move: boolean
}

export type OsDragState = { paths: string[]; target: DropTarget | null }

export interface UseDragAndDropProps {
  wsRef: RefObject<Workspace | null>
  listData: Record<string, ListData | undefined>
  t: Translate
  showToast: (msg: string) => void
  refreshPanes: () => void
  invalidateTree: (dir: string) => void
  invalidateGit: (dir: string) => void
  runOp: (kind: OpKind, sources: string[], options?: RunOpOptions) => void
}

export function useDragAndDrop({
  wsRef,
  listData,
  t,
  showToast,
  refreshPanes,
  invalidateTree,
  invalidateGit,
  runOp,
}: UseDragAndDropProps) {
  const [rowDrag, setRowDrag] = useState<RowDragState | null>(null)
  const [osDrag, setOsDrag] = useState<OsDragState | null>(null)

  const resolveDropTarget = useCallback((x: number, y: number): DropTarget | null => {
    const hit = document.elementFromPoint(x, y)
    const el = hit instanceof Element ? hit.closest<HTMLElement>('[data-drop-dir]') : null
    const dir = el?.dataset.dropDir
    if (!el || !dir) return null
    const rect = el.getBoundingClientRect()
    return { dir, left: rect.left, top: rect.top, width: rect.width, height: rect.height }
  }, [])

  const dropPaths = useCallback(
    (sources: string[], targetDir: string, forced: 'copy' | 'move' | null) => {
      const target = pathKey(targetDir)
      const items = sources.filter(
        (source) =>
          pathKey(source) !== target &&
          pathKey(parentPath(source)) !== target &&
          !underPath(targetDir, source),
      )
      if (!items.length) return
      const dirs = Array.from(new Set([targetDir, ...items.map(parentPath)]))
      const after = () => {
        for (const dir of dirs) {
          invalidateTree(dir)
          invalidateGit(dir)
        }
      }
      if (items.some(isInsideArchive) && !isInsideArchive(targetDir)) {
        void archiveExtractItems(items, targetDir)
          .then((extracted) => {
            invalidateTree(targetDir)
            invalidateGit(targetDir)
            refreshPanes()
            showToast(t('toast.copied', { count: extracted.length }))
          })
          .catch(() => showToast(t('toast.copyFailed')))
        return
      }
      if (forced === 'copy') {
        runOp('copy', items, { targetDir, after })
        return
      }
      if (forced === 'move') {
        runOp('move', items, { targetDir, after })
        return
      }
      const moving = items.filter((source) => sameVolume(source, targetDir))
      const copying = items.filter((source) => !sameVolume(source, targetDir))
      if (copying.length) runOp('copy', copying, { targetDir, after })
      if (moving.length) runOp('move', moving, { targetDir, after })
    },
    [invalidateGit, invalidateTree, refreshPanes, runOp, showToast, t],
  )

  const startRowDrag = useCallback(
    (paneId: string, entry: Entry, ev: ReactPointerEvent) => {
      if (ev.button !== 0) return
      const current = wsRef.current
      const pane = current?.panes[paneId]
      const tab = pane && current ? current.tabs[pane.activeTabId] : undefined
      if (!current || !pane || !tab?.path) return
      const data = listData[tab.id]
      const names = data?.selected.has(entry.name) ? Array.from(data.selected) : [entry.name]
      const sourceDir = tab.path
      const startX = ev.clientX
      const startY = ev.clientY
      let started = false
      let userSelect = ''

      const move = (e: PointerEvent) => {
        if (!started) {
          if (Math.hypot(e.clientX - startX, e.clientY - startY) < 4) return
          started = true
          userSelect = document.body.style.userSelect
          document.body.style.userSelect = 'none'
        }
        if (
          e.clientX < 0 ||
          e.clientY < 0 ||
          e.clientX >= window.innerWidth ||
          e.clientY >= window.innerHeight
        ) {
          detach()
          setRowDrag(null)
          const rawPaths = names.map((name) => joinPath(sourceDir, name))
          void archivePrepareDrag(rawPaths)
            .then((paths) => dragOut(paths))
            .then((effect) => {
              if (effect !== 'move') return
              invalidateTree(sourceDir)
              invalidateGit(sourceDir)
              refreshPanes()
            })
            .catch(() => showToast(t('drag.outFailed')))
          return
        }
        setRowDrag({
          names,
          sourceDir,
          x: e.clientX,
          y: e.clientY,
          started: true,
          target: resolveDropTarget(e.clientX, e.clientY),
          copy: e.ctrlKey || e.metaKey,
          move: e.shiftKey && !(e.ctrlKey || e.metaKey),
        })
      }

      let detach = () => {}
      const onKey = (e: KeyboardEvent) => {
        if (e.key === 'Escape') finish(null)
      }
      const finish = (e: PointerEvent | null) => {
        detach()
        setRowDrag(null)
        if (!e) return
        const target = resolveDropTarget(e.clientX, e.clientY)
        if (!target) return
        const forced = e.ctrlKey || e.metaKey ? 'copy' : e.shiftKey ? 'move' : null
        dropPaths(
          names.map((name) => joinPath(sourceDir, name)),
          target.dir,
          forced,
        )
      }
      const up = (e: PointerEvent) => finish(e)
      const cancel = () => finish(null)
      detach = () => {
        window.removeEventListener('pointermove', move)
        window.removeEventListener('pointerup', up)
        window.removeEventListener('pointercancel', cancel)
        window.removeEventListener('keydown', onKey)
        if (started) document.body.style.userSelect = userSelect
      }
      window.addEventListener('pointermove', move)
      window.addEventListener('pointerup', up)
      window.addEventListener('pointercancel', cancel)
      window.addEventListener('keydown', onKey)
    },
    [dropPaths, invalidateGit, invalidateTree, listData, refreshPanes, resolveDropTarget, showToast, t, wsRef],
  )

  useEffect(() => {
    const mods = { ctrl: false, shift: false }
    const onModifier = (e: KeyboardEvent) => {
      mods.ctrl = e.ctrlKey
      mods.shift = e.shiftKey
    }
    let paths: string[] = []
    let disposed = false
    let unlisten: (() => void) | undefined
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload
        if (payload.type === 'leave') {
          paths = []
          setOsDrag(null)
          return
        }
        const dpr = window.devicePixelRatio || 1
        const target = resolveDropTarget(payload.position.x / dpr, payload.position.y / dpr)
        if (payload.type === 'enter') paths = payload.paths
        if (payload.type === 'drop') {
          const dropped = payload.paths.filter((path) => isFsPath(path))
          setOsDrag(null)
          if (!target || !dropped.length) return
          const forced = mods.ctrl ? 'copy' : mods.shift ? 'move' : null
          dropPaths(dropped, target.dir, forced)
          return
        }
        setOsDrag({ paths, target })
      })
      .then((fn) => {
        if (disposed) fn()
        else unlisten = fn
      })
    window.addEventListener('keydown', onModifier)
    window.addEventListener('keyup', onModifier)
    return () => {
      disposed = true
      unlisten?.()
      window.removeEventListener('keydown', onModifier)
      window.removeEventListener('keyup', onModifier)
    }
  }, [dropPaths, resolveDropTarget])

  return {
    rowDrag,
    setRowDrag,
    osDrag,
    setOsDrag,
    resolveDropTarget,
    dropPaths,
    startRowDrag,
  }
}
