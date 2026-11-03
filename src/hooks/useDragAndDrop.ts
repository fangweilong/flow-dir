import { useCallback, useEffect, useRef, useState } from 'react'
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
  dropModifiers,
} from '../api'
import {
  isFsPath,
  isArchiveMember,
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
  // The pointerdown handler runs in the same tick as the selection update, so the
  // `listData` captured in that closure is one render behind. The drag payload is read
  // from this ref once the gesture actually starts, after React has committed.
  const listDataRef = useRef(listData)
  useEffect(() => {
    listDataRef.current = listData
  }, [listData])

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
      if (items.some(isArchiveMember) && !isInsideArchive(targetDir)) {
        void archiveExtractItems(items, targetDir)
          .then((extracted) => {
            invalidateTree(targetDir)
            invalidateGit(targetDir)
            refreshPanes()
            showToast(t('toast.copied', { count: extracted.length }))
          })
          .catch(() => {
            // A failed batch still leaves the items extracted before the error; refresh so
            // the list matches what is actually on disk.
            invalidateTree(targetDir)
            refreshPanes()
            showToast(t('toast.copyFailed'))
          })
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
      const sourceDir = tab.path
      // The pointerdown handler collapses a multi-selection to the pressed row in the same
      // tick, so the committed ref no longer holds what the user sees while dragging.
      const atDown = listData[tab.id]
      const startX = ev.clientX
      const startY = ev.clientY
      let started = false
      // The gesture ends exactly once: either handed to OLE or resolved in-app.
      let finished = false
      let handedOff = false
      let userSelect = ''
      // Prefer the selection captured before pointerdown (what the user saw when pressing
      // the row); the committed ref only helps when the pressed row was not selected yet.
      const selectedNames = () => {
        if (atDown?.selected.has(entry.name)) return Array.from(atDown.selected)
        const live = listDataRef.current[tab.id]
        return live?.selected.has(entry.name) ? Array.from(live.selected) : [entry.name]
      }

      let detach = () => {}
      /**
       * Hands the gesture to OLE once it leaves the window. WebView2 does not always
       * report out-of-window pointer coordinates, so both the out-of-bounds move and
       * `pointerleave` funnel here; the first one wins.
       */
      const handOff = () => {
        if (handedOff || !started) return
        handedOff = true
        detach()
        setRowDrag(null)
        const rawPaths = selectedNames().map((name) => joinPath(sourceDir, name))
        void archivePrepareDrag(rawPaths)
          .then((paths) => dragOut(paths))
          .then((effect) => {
            if (effect !== 'move') return
            invalidateTree(sourceDir)
            invalidateGit(sourceDir)
            refreshPanes()
          })
          .catch(() => showToast(t('drag.outFailed')))
      }

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
          handOff()
          return
        }
        setRowDrag({
          names: selectedNames(),
          sourceDir,
          x: e.clientX,
          y: e.clientY,
          started: true,
          target: resolveDropTarget(e.clientX, e.clientY),
          copy: e.ctrlKey || e.metaKey,
          move: e.shiftKey && !(e.ctrlKey || e.metaKey),
        })
      }

      const onKey = (e: KeyboardEvent) => {
        if (e.key === 'Escape') finish(null)
      }
      const finish = (e: PointerEvent | null) => {
        if (finished || handedOff) return
        finished = true
        detach()
        setRowDrag(null)
        // A click without any pointermove is a selection, not a drop.
        if (!started || !e) return
        const target = resolveDropTarget(e.clientX, e.clientY)
        if (!target) return
        const forced = e.ctrlKey || e.metaKey ? 'copy' : e.shiftKey ? 'move' : null
        dropPaths(
          selectedNames().map((name) => joinPath(sourceDir, name)),
          target.dir,
          forced,
        )
      }
      const up = (e: PointerEvent) => finish(e)
      const out = (e: PointerEvent) => {
        if (
          e.clientX < 0 ||
          e.clientY < 0 ||
          e.clientX >= window.innerWidth ||
          e.clientY >= window.innerHeight
        ) {
          handOff()
        }
      }
      const cancel = () => {
        if (!started) {
          finish(null)
          return
        }
        // Losing capture at the window edge can beat `pointerleave`; wait a frame for
        // the leave to hand the gesture to OLE before treating it as cancelled.
        window.setTimeout(() => finish(null), 60)
      }
      detach = () => {
        window.removeEventListener('pointermove', move)
        window.removeEventListener('pointerup', up)
        window.removeEventListener('pointercancel', cancel)
        window.removeEventListener('pointerout', out)
        window.removeEventListener('keydown', onKey)
        document.documentElement.removeEventListener('pointerleave', handOff)
        if (started) document.body.style.userSelect = userSelect
      }
      window.addEventListener('pointermove', move)
      window.addEventListener('pointerup', up)
      window.addEventListener('pointercancel', cancel)
      window.addEventListener('pointerout', out)
      window.addEventListener('keydown', onKey)
      // `pointerleave` covers the case where WebView2 stops reporting moves at the
      // window edge, which would otherwise leave the drag inside the app forever.
      document.documentElement.addEventListener('pointerleave', handOff)
    },
    [dropPaths, invalidateGit, invalidateTree, listData, refreshPanes, resolveDropTarget, showToast, t, wsRef],
  )

  useEffect(() => {
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
          // The drop payload carries no keyboard state and the webview saw no keydown
          // while another process was dragging, so ask the OS which modifiers are held.
          void dropModifiers()
            .then((mask) => {
              const forced = mask & 1 ? 'copy' : mask & 2 ? 'move' : null
              dropPaths(dropped, target.dir, forced)
            })
            .catch(() => dropPaths(dropped, target.dir, null))
          return
        }
        setOsDrag({ paths, target })
      })
      .then((fn) => {
        if (disposed) fn()
        else unlisten = fn
      })
    return () => {
      disposed = true
      unlisten?.()
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
