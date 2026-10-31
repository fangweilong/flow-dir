import { useCallback, useEffect, useRef, useState } from 'react'
import type { MutableRefObject } from 'react'
import type { OpKind, OpProgress } from '../types'
import type { Settings } from '../persist'
import { errorKey } from '../i18n'
import type { Translate } from '../i18n'
import type { SyncAction } from '../ComparePanel'
import {
  archiveCompress,
  archiveExtract,
  notifyOp,
  opRetryElevated,
  opStart,
  restoreRecycle,
  taskbarProgress,
  uniquePaths,
  COMPRESS_EXTENSIONS,
  type CompressFormat,
} from '../api'
import { baseName, joinPath, parentPath } from '../paths'
import { undoEntryFor, UNDO_LIMIT } from '../undo'
import type { OpRequestShape, UndoEntry, UndoPayload } from '../undo'
export const errorCode = (err: unknown): string =>
  err && typeof err === 'object' && 'code' in err && typeof err.code === 'string'
    ? err.code
    : String(err)

function uid(prefix: string): string {
  return `${prefix}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`
}

export type RunOpOptions = {
  targetDir?: string
  newName?: string
  newNames?: string[]
  permanent?: boolean
  overwrite?: boolean
  after?: () => void
  record?: boolean
  dirsAfter?: string[]
  onDone?: (ok: boolean) => void
}

export interface UseFileOperationsProps {
  settingsRef: MutableRefObject<Settings>
  t: Translate
  showToast: (msg: string, timeout?: number) => void
  refreshPanes: () => void
  invalidateTree: (dir: string) => void
  invalidateGit: (dir: string) => void
}

export function useFileOperations({
  settingsRef,
  t,
  showToast,
  refreshPanes,
  invalidateTree,
  invalidateGit,
}: UseFileOperationsProps) {
  const [ops, setOps] = useState<Record<string, OpProgress>>({})
  const [elevating, setElevating] = useState<string | null>(null)
  const [historyEpoch, setHistoryEpoch] = useState(0)

  const [passwordPrompt, setPasswordPrompt] = useState<{
    archive: string
    dest: string
    wrong: boolean
  } | null>(null)
  const [extractTarget, setExtractTarget] = useState<{ archive: string; initial: string } | null>(
    null,
  )
  const [compressTarget, setCompressTarget] = useState<{
    paths: string[]
    dir: string
    format: CompressFormat
    name: string
  } | null>(null)

  const undoStack = useRef<UndoEntry[]>([])
  const redoStack = useRef<UndoEntry[]>([])
  const undoCounter = useRef(0)
  const lastIrreversible = useRef(false)
  const opRequests = useRef<Record<string, OpRequestShape & { opId: string }>>({})
  const taskbarWant = useRef<{ state: string; done: number; total: number } | null>(null)
  const taskbarTimer = useRef<number | null>(null)
  const taskbarSentAt = useRef(0)
  const undoLimit = useCallback(
    () => settingsRef.current.undoLimit ?? UNDO_LIMIT,
    [settingsRef],
  )

  const pushUndo = useCallback(
    (payload: UndoPayload, request: OpRequestShape, dirs: string[]) => {
      undoCounter.current += 1
      undoStack.current.push({
        ...payload,
        id: undoCounter.current,
        at: Date.now(),
        request,
        dirs,
      })
      const limit = undoLimit()
      while (undoStack.current.length > limit) undoStack.current.shift()
      redoStack.current.length = 0
      setHistoryEpoch((epoch) => epoch + 1)
    },
    [undoLimit],
  )

  const runOp = useCallback(
    (kind: OpKind, sources: string[], options?: RunOpOptions) => {
      if (!sources.length) return
      const opId = uid('op')
      const request: OpRequestShape = {
        kind,
        sources,
        targetDir: options?.targetDir,
        newName: options?.newName,
        newNames: options?.newNames,
        permanent: options?.permanent,
        overwrite: options?.overwrite,
      }
      opRequests.current[opId] = { opId, ...request }
      const recordEntry = (payload: UndoPayload | null, dirs: string[]) => {
        if (options?.record === false) return
        if (!payload) {
          if (kind === 'delete' && options?.permanent) lastIrreversible.current = true
          return
        }
        lastIrreversible.current = false
        pushUndo(payload, request, dirs)
      }
      setOps((prev) => ({
        ...prev,
        [opId]: {
          opId,
          phase: kind,
          current: 0,
          total: sources.length,
          name: null,
          work: 0,
          workTotal: 0,
          failedCount: 0,
          failed: [],
          done: false,
          aborted: false,
          error: null,
        },
      }))
      const drop = (delay: number) =>
        window.setTimeout(() => {
          delete opRequests.current[opId]
          setOps((prev) => {
            const next = { ...prev }
            delete next[opId]
            return next
          })
        }, delay)

      const remind = (text: string) => {
        if (!settingsRef.current.notifyOps || document.hasFocus()) return
        void notifyOp(t('notify.title'), text).catch(() => undefined)
      }
      const announce = (text: string) => {
        showToast(text)
        remind(text)
      }
      opStart(
        {
          opId,
          kind,
          sources,
          targetDir: options?.targetDir,
          newName: options?.newName,
          newNames: options?.newNames,
          permanent: options?.permanent,
          overwrite: options?.overwrite,
        },
        (progress) => {
          setOps((prev) => (prev[opId] ? { ...prev, [opId]: progress } : prev))
          if (!progress.done) return
          const denied =
            progress.error?.code === 'ACCESS_DENIED' ||
            progress.failed.some((failure) => failure.code === 'ACCESS_DENIED')
          if (progress.error) {
            announce(t('op.failed', { code: t(errorKey(progress.error.code)) }))
            options?.onDone?.(false)
            if (!denied) drop(5000)
            return
          }
          if (progress.aborted) {
            announce(t('op.canceled'))
            options?.onDone?.(false)
            drop(2500)
            return
          }
          if (progress.failedCount > 0) {
            const failed = progress.failedCount
            const ok = Math.max(0, progress.total - failed)
            announce(t('op.partial', { ok, failed }))
            recordEntry(
              undoEntryFor(kind, sources, {
                ...options,
                skip: progress.failed.map((failure) => failure.path),
              }),
              options?.dirsAfter ?? [],
            )
            options?.after?.()
            refreshPanes()
            options?.onDone?.(false)
            drop(denied ? 600_000 : 15000)
            return
          }
          recordEntry(undoEntryFor(kind, sources, options), options?.dirsAfter ?? [])
          options?.after?.()
          refreshPanes()
          remind(t('op.done'))
          options?.onDone?.(true)
          drop(800)
        },
      ).catch((e: unknown) => {
        showToast(t('op.failed', { code: t(errorKey(errorCode(e))) }))
        options?.onDone?.(false)
        drop(4000)
      })
    },
    [pushUndo, refreshPanes, settingsRef, showToast, t],
  )

  const retryElevated = useCallback(
    (opId: string) => {
      const request = opRequests.current[opId]
      if (!request || elevating) return
      setElevating(opId)
      showToast(t('op.elevating'))
      opRetryElevated(request)
        .then((report) => {
          refreshPanes()
          if (report.error) {
            showToast(t('op.failed', { code: t(errorKey(report.error.code)) }))
          } else if (report.aborted) {
            showToast(t('op.canceled'))
          } else if (report.failedTotal > 0) {
            showToast(
              t('op.partial', {
                ok: Math.max(0, request.sources.length - report.failedTotal),
                failed: report.failedTotal,
              }),
            )
          } else {
            showToast(t('op.done'))
            delete opRequests.current[opId]
            setOps((prev) => {
              if (!(opId in prev)) return prev
              const next = { ...prev }
              delete next[opId]
              return next
            })
          }
        })
        .catch((err: unknown) => {
          const code = errorCode(err)
          showToast(code === 'ELEVATION_CANCELED' ? t('op.canceled') : t('op.failed', { code: t(errorKey(code)) }))
        })
        .finally(() => setElevating(null))
    },
    [elevating, refreshPanes, showToast, t],
  )

  const extractArchive = useCallback(
    (archive: string, dest: string, password?: string) => {
      const opId = `extract-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
      setOps((prev) => ({
        ...prev,
        [opId]: {
          opId,
          phase: 'extract',
          current: 0,
          total: 0,
          name: null,
          work: 0,
          workTotal: 0,
          failedCount: 0,
          failed: [],
          done: false,
          aborted: false,
          error: null,
        },
      }))
      const drop = (delay: number) =>
        window.setTimeout(() => {
          setOps((prev) => {
            const next = { ...prev }
            delete next[opId]
            return next
          })
        }, delay)
      archiveExtract(opId, archive, dest, password ?? null, (progress) => {
        setOps((prev) => (prev[opId] ? { ...prev, [opId]: progress } : prev))
        if (!progress.done) return
        if (progress.error) {
          const code = progress.error.code
          if (code === 'PASSWORD_REQUIRED' || code === 'WRONG_PASSWORD') {
            setPasswordPrompt({ archive, dest, wrong: code === 'WRONG_PASSWORD' })
            drop(3000)
            return
          }
          showToast(t('op.failed', { code: t(errorKey(code)) }))
          drop(5000)
          return
        }
        if (progress.aborted) {
          showToast(t('op.canceled'))
          drop(2500)
          return
        }
        refreshPanes()
        if (progress.failedCount > 0) {
          showToast(
            t('op.partial', {
              ok: Math.max(0, progress.total - progress.failedCount),
              failed: progress.failedCount,
            }),
            8000,
          )
          drop(30000)
          return
        }
        showToast(t('op.done'))
        drop(800)
      }).catch((e: unknown) => {
        showToast(t('op.failed', { code: t(errorKey(errorCode(e))) }))
        drop(4000)
      })
    },
    [refreshPanes, showToast, t],
  )

  const runCompress = useCallback(
    (paths: string[], dest: string, format: CompressFormat, level: number, password: string | null) => {
      const opId = `compress-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
      setOps((prev) => ({
        ...prev,
        [opId]: {
          opId,
          phase: 'compress',
          current: 0,
          total: 0,
          name: null,
          work: 0,
          workTotal: 0,
          failedCount: 0,
          failed: [],
          done: false,
          aborted: false,
          error: null,
        },
      }))
      const drop = (delay: number) =>
        window.setTimeout(() => {
          setOps((prev) => {
            const next = { ...prev }
            delete next[opId]
            return next
          })
        }, delay)
      archiveCompress(opId, paths, dest, format, level, password, (progress) => {
        setOps((prev) => (prev[opId] ? { ...prev, [opId]: progress } : prev))
        if (!progress.done) return
        if (progress.error) {
          showToast(t('op.failed', { code: t(errorKey(progress.error.code)) }))
          drop(5000)
          return
        }
        if (progress.aborted) {
          showToast(t('op.canceled'))
          drop(2500)
          return
        }
        refreshPanes()
        showToast(t('op.done'))
        drop(800)
      }).catch((e: unknown) => {
        showToast(t('op.failed', { code: t(errorKey(errorCode(e))) }))
        drop(4000)
      })
    },
    [refreshPanes, showToast, t],
  )

  const requestCompress = useCallback(
    (paths: string[], dir: string, format: CompressFormat, auto: boolean) => {
      const base = paths.length === 1 ? baseName(paths[0]) : baseName(dir) || 'archive'
      const extension = COMPRESS_EXTENSIONS[format]
      void uniquePaths([joinPath(dir, `${base}${extension}`)])
        .then(([dest]) => {
          if (auto) runCompress(paths, dest, format, 6, null)
          else setCompressTarget({ paths, dir, format, name: baseName(dest) })
        })
        .catch((e: unknown) => showToast(t('op.failed', { code: t(errorKey(errorCode(e))) })))
    },
    [runCompress, showToast, t],
  )

  const runStep = useCallback(
    (kind: OpKind, sources: string[], options: RunOpOptions = {}) => {
      const { promise, resolve } = Promise.withResolvers<boolean>()
      runOp(kind, sources, { ...options, record: false, onDone: resolve })
      return promise
    },
    [runOp],
  )

  const runSyncActions = useCallback(
    (actions: SyncAction[]) => {
      const { promise, resolve } = Promise.withResolvers<void>()
      const copies = new Map<string, string[]>()
      const deletes: string[] = []
      for (const action of actions) {
        if (action.kind === 'copy') {
          const list = copies.get(action.targetDir) ?? []
          list.push(action.source)
          copies.set(action.targetDir, list)
        } else {
          deletes.push(...action.paths)
        }
      }
      const steps: (() => void)[] = []
      for (const [targetDir, sources] of copies) {
        steps.push(() =>
          runOp('copy', sources, { targetDir, overwrite: true, onDone: runNext }),
        )
      }
      if (deletes.length) {
        steps.push(() => runOp('delete', deletes, { onDone: runNext }))
      }
      let index = 0
      function runNext() {
        if (index >= steps.length) {
          resolve()
          return
        }
        steps[index++]()
      }
      runNext()
      return promise
    },
    [runOp],
  )

  const applyUndo = useCallback(
    async (entry: UndoEntry): Promise<boolean> => {
      const afterDirs = (dirs: string[]) => () => {
        for (const dir of new Set(dirs)) {
          invalidateTree(dir)
          invalidateGit(dir)
        }
      }
      switch (entry.kind) {
        case 'rename': {
          const dirs = entry.items.map((item) => parentPath(item.path))
          return runStep(
            'rename',
            entry.items.map((item) => joinPath(parentPath(item.path), item.newName)),
            { newNames: entry.items.map((item) => item.oldName), after: afterDirs(dirs) },
          )
        }
        case 'move': {
          const byDir = new Map<string, string[]>()
          for (const item of entry.items) {
            const dir = parentPath(item.from)
            byDir.set(dir, [...(byDir.get(dir) ?? []), item.to])
          }
          const results = await Promise.all(
            [...byDir].map(([dir, paths]) =>
              runStep('move', paths, { targetDir: dir, after: afterDirs([dir]) }),
            ),
          )
          return results.every(Boolean)
        }
        case 'copy':
          return runStep('delete', entry.created, {
            after: afterDirs(entry.created.map(parentPath)),
          })
        case 'delete': {
          try {
            const report = await restoreRecycle(entry.paths)
            if (report.missing.length) {
              showToast(
                t('undo.partial', { ok: report.restored.length, failed: report.missing.length }),
              )
            } else {
              showToast(t('undo.done'))
            }
            for (const path of report.restored) invalidateTree(parentPath(path))
            refreshPanes()
            return report.restored.length > 0
          } catch (e: unknown) {
            showToast(t('op.failed', { code: t(errorKey(errorCode(e))) }))
            return false
          }
        }
      }
    },
    [invalidateGit, invalidateTree, refreshPanes, runStep, showToast, t],
  )

  const undoLast = useCallback(() => {
    const entry = undoStack.current[undoStack.current.length - 1]
    if (!entry) {
      showToast(t(lastIrreversible.current ? 'undo.irreversible' : 'undo.empty'))
      return
    }
    void applyUndo(entry).then((ok) => {
      if (!ok) return
      undoStack.current.pop()
      redoStack.current.push(entry)
      while (redoStack.current.length > undoLimit()) redoStack.current.shift()
      setHistoryEpoch((epoch) => epoch + 1)
    })
  }, [applyUndo, showToast, t, undoLimit])

  const redoLast = useCallback(() => {
    const entry = redoStack.current[redoStack.current.length - 1]
    if (!entry) {
      showToast(t('redo.empty'))
      return
    }
    const { request } = entry
    const afterDirs = () => {
      for (const dir of new Set(entry.dirs)) {
        invalidateTree(dir)
        invalidateGit(dir)
      }
    }
    void runStep(request.kind, request.sources, {
      targetDir: request.targetDir,
      newName: request.newName,
      newNames: request.newNames,
      overwrite: request.overwrite,
      after: afterDirs,
    }).then((ok: boolean) => {
      if (!ok) return
      redoStack.current.pop()
      undoStack.current.push(entry)
      while (undoStack.current.length > undoLimit()) undoStack.current.shift()
      setHistoryEpoch((epoch) => epoch + 1)
    })
  }, [invalidateGit, invalidateTree, runStep, showToast, t, undoLimit])

  const undoTo = useCallback(
    async (targetId: number) => {
      const index = undoStack.current.findIndex((entry) => entry.id === targetId)
      if (index === -1) return
      const count = undoStack.current.length - index
      for (let i = 0; i < count; i += 1) {
        const entry = undoStack.current[undoStack.current.length - 1]
        if (!entry) break
        const ok = await applyUndo(entry)
        if (!ok) break
        undoStack.current.pop()
        redoStack.current.push(entry)
        while (redoStack.current.length > undoLimit()) redoStack.current.shift()
      }
      setHistoryEpoch((epoch) => epoch + 1)
    },
    [applyUndo, undoLimit],
  )

  useEffect(() => {
    const active = Object.values(ops).filter((op) => !op.done)
    const state = !active.length
      ? 'none'
      : active.some((op) => op.failedCount > 0 || op.error)
        ? 'error'
        : active.some((op) => op.aborted)
          ? 'paused'
          : 'normal'
    let done = 0
    let total = 0
    for (const op of active) {
      const each = Math.max(op.total, 1)
      done += Math.min(op.current, each)
      total += each
    }
    taskbarWant.current = { state, done, total }
    const flush = () => {
      taskbarTimer.current = null
      const want = taskbarWant.current
      if (!want) return
      taskbarSentAt.current = Date.now()
      void taskbarProgress(want.state, want.done, want.total).catch(() => undefined)
    }
    if (taskbarTimer.current) return
    const wait = 160 - (Date.now() - taskbarSentAt.current)
    if (wait <= 0) flush()
    else taskbarTimer.current = window.setTimeout(flush, wait)
  }, [ops])

  return {
    ops,
    setOps,
    elevating,
    runOp,
    retryElevated,
    extractArchive,
    runCompress,
    requestCompress,
    runSyncActions,
    undoLast,
    redoLast,
    undoTo,
    undoStack,
    redoStack,
    historyEpoch,
    passwordPrompt,
    setPasswordPrompt,
    extractTarget,
    setExtractTarget,
    compressTarget,
    setCompressTarget,
  }
}
