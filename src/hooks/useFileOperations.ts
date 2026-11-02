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
  pathIsDir,
  pathsExist,
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
  const opRequests = useRef<Record<string, OpRequestShape & { opId: string; record?: boolean }>>({})
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
      opRequests.current[opId] = { opId, ...request, record: options?.record }
      const recordEntry = (
        payload: UndoPayload | null,
        dirs: string[],
        recorded: OpRequestShape = request,
      ) => {
        if (options?.record === false) return
        if (!payload) {
          if (kind === 'delete' && options?.permanent) lastIrreversible.current = true
          return
        }
        lastIrreversible.current = false
        pushUndo(payload, recorded, dirs)
      }
      // Redo replays `request.sources`, so a partial or cancelled batch must record only
      // the items that landed. Otherwise Ctrl+Y re-runs items the user never completed,
      // and the extra effects are missing from the entry pushed back onto the stack.
      const narrow = (keep: number[]): OpRequestShape => ({
        ...request,
        sources: keep.map((index) => sources[index]),
        newNames: request.newNames ? keep.map((index) => request.newNames![index]) : undefined,
      })
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
            // Items that finished before the cancel are already on disk; without a record
            // Ctrl+Z cannot reverse them. `succeeded` is authoritative (the failure list
            // is capped), and `undoEntryFor` keeps positional rename names aligned.
            const done = [...new Set(progress.succeeded ?? [])].sort((a, b) => a - b)
            if (done.length) {
              const landed = new Set(done)
              recordEntry(
                undoEntryFor(kind, sources, {
                  ...options,
                  skip: sources.filter((_, index) => !landed.has(index)),
                }),
                options?.dirsAfter ?? [],
                narrow(done),
              )
            }
            options?.after?.()
            refreshPanes()
            announce(t('op.canceled'))
            options?.onDone?.(false)
            drop(2500)
            return
          }
          if (progress.failedCount > 0) {
            const failed = progress.failedCount
            const ok = Math.max(0, progress.total - failed)
            announce(t('op.partial', { ok, failed }))
            // The backend caps the failure list; past that point the index list is the
            // only way to know which items landed (and must not be recorded as undoable).
            const succeeded = progress.succeeded
            const failedIndices = progress.failedIndices
            let keep = succeeded ?? []
            if (!keep.length) {
              if (failedIndices?.length) {
                const failedSet = new Set(failedIndices)
                keep = sources.map((_, index) => index).filter((index) => !failedSet.has(index))
              } else if (
                progress.failed.length === progress.failedCount &&
                // A batch-level failure carries an empty path; treating it as a path
                // match would mark every source as landed and record a phantom undo.
                progress.failed.every((failure) => failure.path)
              ) {
                // The list is complete, so the paths identify the landed items. When it is
                // capped, deriving from it would record items that never landed.
                const failedSet = new Set(
                  progress.failed.map((failure) => failure.path.toLowerCase()),
                )
                keep = sources
                  .map((_, index) => index)
                  .filter((index) => !failedSet.has(sources[index].toLowerCase()))
              }
            }
            const landed = new Set(keep)
            recordEntry(
              undoEntryFor(kind, sources, {
                ...options,
                skip: sources.filter((_, index) => !landed.has(index)),
              }),
              options?.dirsAfter ?? [],
              narrow(keep),
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
      // Replay only the items that were denied. A rename keeps its full batch because its
      // per-item names are positional.
      const failedPaths = new Set(
        (ops[opId]?.failed ?? []).map((failure) => failure.path.toLowerCase()),
      )
      const failedIndices = ops[opId]?.failedIndices ?? []
      const canSubset = request.kind !== 'rename' && !request.newName && !request.newNames
      // The failure list is capped at 200; the index list is not, so a large denied batch
      // retries completely instead of silently dropping the tail.
      const subsetSources = !canSubset
        ? request.sources
        : failedIndices.length
          ? failedIndices
              .map((index) => request.sources[index])
              .filter((source): source is string => Boolean(source))
          : request.sources.filter((source) => failedPaths.has(source.toLowerCase()))
      // Only the shell-level error carries no per-item paths; then the whole request is
      // the right unit to retry.
      const retry = subsetSources.length ? { ...request, sources: subsetSources } : request
      setElevating(opId)
      showToast(t('op.elevating'))
      opRetryElevated(retry)
        .then((report) => {
          refreshPanes()
          const failed = new Set(report.failed.map((failure) => failure.path.toLowerCase()))
          const succeeded = report.succeeded ?? []
          const reportedIndices = report.failedIndices ?? []
          let keep = succeeded
          if (!keep.length && !report.aborted) {
            if (reportedIndices.length) {
              const failedSet = new Set(reportedIndices)
              keep = retry.sources
                .map((_, index) => index)
                .filter((index) => !failedSet.has(index))
            } else if (
              !report.aborted &&
              report.failedTotal === report.failed.length &&
              // A batch-level failure is reported with an empty path; a path match would
              // otherwise leave every source marked as landed.
              report.failed.every((failure) => failure.path)
            ) {
              keep = retry.sources
                .map((_, index) => index)
                .filter((index) => !failed.has(retry.sources[index].toLowerCase()))
            }
          }
          const completed = keep.map((index) => retry.sources[index])
          // The elevated run does not go through `runOp`, so its history entry has to be
          // recorded here or Ctrl+Z would skip over it.
          // An inverse operation replayed by undo/redo must not add a new history entry.
          if (!report.error && completed.length && request.record !== false) {
            const dirs = Array.from(
              new Set([
                ...(retry.targetDir ? [retry.targetDir] : []),
                ...completed.map((source) => parentPath(source)),
              ]),
            )
            const landed = new Set(keep)
            const entry = undoEntryFor(retry.kind, retry.sources, {
              targetDir: retry.targetDir,
              newName: retry.newName,
              newNames: retry.newNames,
              permanent: retry.permanent,
              skip: retry.sources.filter((_, index) => !landed.has(index)),
            })
            if (entry) {
              pushUndo(
                entry,
                {
                  ...retry,
                  sources: completed,
                  newNames: retry.newNames
                    ? keep.map((index) => retry.newNames![index])
                    : undefined,
                },
                dirs,
              )
            } else if (retry.kind === 'delete' && retry.permanent) {
              lastIrreversible.current = true
            }
          }
          if (report.error) {
            showToast(t('op.failed', { code: t(errorKey(report.error.code)) }))
          } else if (report.aborted) {
            // Items completed before the cancel are already on disk and already in the
            // undo stack; replaying them would duplicate copies or fail every move.
            // Renames are left whole because their names are positional and the backend
            // re-arms the complete request.
            const landed = new Set(keep)
            const remaining = retry.sources
              .map((_, index) => index)
              .filter((index) => !landed.has(index))
            if (
              remaining.length &&
              remaining.length < retry.sources.length &&
              retry.kind !== 'rename' &&
              !retry.newName &&
              !retry.newNames
            ) {
              opRequests.current[opId] = {
                ...retry,
                sources: remaining.map((index) => retry.sources[index]),
              }
              setOps((prev) => {
                const current = prev[opId]
                if (!current) return prev
                return {
                  ...prev,
                  [opId]: {
                    ...current,
                    // Every remaining source still has to run, so their indices restart at 0.
                    failedIndices: remaining.map((_, index) => index),
                  },
                }
              })
            }
            showToast(t('op.canceled'))
          } else if (report.failedTotal > 0) {
            showToast(
              t('op.partial', {
                ok: Math.max(0, retry.sources.length - report.failedTotal),
                failed: report.failedTotal,
              }),
            )
            // A second "retry elevated" click must only replay what still failed, or the
            // items that already landed would run a second time.
            const landed = new Set(keep)
            const failedIdxNew = retry.sources
              .map((_, index) => index)
              .filter((index) => !landed.has(index))
            opRequests.current[opId] = {
              ...retry,
              sources: failedIdxNew.map((index) => retry.sources[index]),
              newNames: retry.newNames
                ? failedIdxNew.map((index) => retry.newNames![index])
                : undefined,
            }
            setOps((prev) => {
              const current = prev[opId]
              if (!current) return prev
              return {
                ...prev,
                [opId]: {
                  ...current,
                  failed: report.failed,
                  failedCount: report.failedTotal,
                  // `opRequests` now holds only the failed sources, so their positions
                  // restart at 0: keeping the old indices would skip items on the next retry.
                  failedIndices: failedIdxNew.map((_, index) => index),
                },
              }
            })
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
    [elevating, ops, pushUndo, refreshPanes, showToast, t],
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
        if (progress.failedCount > 0) {
          // Unreadable entries are skipped and still produce an archive; reporting plain
          // "done" would hide that files are missing from it.
          showToast(
            t('op.partial', {
              ok: Math.max(0, progress.total - progress.failedCount),
              failed: progress.failedCount,
            }),
          )
          drop(15000)
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

  const requestCompress = useCallback(
    (paths: string[], dir: string, format: CompressFormat) => {
      const extension = COMPRESS_EXTENSIONS[format]
      void (async () => {
        let base = baseName(dir) || 'archive'
        if (paths.length === 1) {
          const full = baseName(paths[0])
          // Files compress to `report.zip`, not `report.docx.zip`; a folder keeps its full
          // name because dots are legitimate there.
          const dot = full.lastIndexOf('.')
          base =
            dot > 0 && dot < full.length - 1 && !(await pathIsDir(paths[0]))
              ? full.slice(0, dot)
              : full
        }
        const [dest] = await uniquePaths([joinPath(dir, `${base}${extension}`)])
        setCompressTarget({ paths, dir, format, name: baseName(dest) })
      })().catch((e: unknown) =>
        showToast(t('op.failed', { code: t(errorKey(errorCode(e))) })),
      )
    },
    [showToast, t],
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
          const sources = entry.items.map((item) => joinPath(parentPath(item.path), item.newName))
          // A previous attempt may have restored some names already; replaying them fails
          // with NOT_FOUND and left the entry blocking the whole history forever.
          const flags = await pathsExist(sources)
          const pending = entry.items.filter((_, index) => flags[index])
          if (!pending.length) {
            showToast(t('undo.done'))
            return true
          }
          const dirs = pending.map((item) => parentPath(item.path))
          return runStep(
            'rename',
            pending.map((item) => joinPath(parentPath(item.path), item.newName)),
            { newNames: pending.map((item) => item.oldName), after: afterDirs(dirs) },
          )
        }
        case 'move': {
          const flags = await pathsExist(entry.items.map((item) => item.to))
          const pending = entry.items.filter((_, index) => flags[index])
          if (!pending.length) {
            showToast(t('undo.done'))
            return true
          }
          const byDir = new Map<string, string[]>()
          for (const item of pending) {
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
        case 'copy': {
          // A copy the user already removed is already undone; deleting nothing must not
          // wedge the whole history.
          const flags = await pathsExist(entry.created)
          const existing = entry.created.filter((_, index) => flags[index])
          if (!existing.length) {
            showToast(t('undo.done'))
            return true
          }
          return runStep('delete', existing, {
            after: afterDirs(existing.map(parentPath)),
          })
        }
        case 'delete': {
          try {
            const report = await restoreRecycle(entry.paths)
            // `missing` means the item left the bin and no retry can bring it back, so it
            // counts as handled; `failed` means it is still there but the shell verb
            // refused (locked target, permissions), so a retry can work.
            const handled = report.missing.length
            const retryable = report.failed.length
            if (handled + retryable > 0) {
              showToast(t('undo.partial', { ok: report.restored.length, failed: handled + retryable }))
            } else {
              showToast(t('undo.done'))
            }
            for (const path of report.restored) invalidateTree(parentPath(path))
            refreshPanes()
            // A retryable failure keeps the entry on the undo stack: popping it here would
            // drop the only handle on items still sitting in the bin. Missing entries never
            // block older history, since no later attempt could restore them.
            return retryable === 0
          } catch (e: unknown) {
            showToast(t('op.failed', { code: t(errorKey(errorCode(e))) }))
            return false
          }
        }
      }
    },
    [invalidateGit, invalidateTree, refreshPanes, runStep, showToast, t],
  )

  // One reversal at a time: two overlapping runs would race on the same paths and could
  // pop the wrong entry off the stack.
  const undoBusy = useRef(false)

  const undoLast = useCallback(() => {
    if (undoBusy.current) return
    const entry = undoStack.current[undoStack.current.length - 1]
    if (!entry) {
      showToast(t(lastIrreversible.current ? 'undo.irreversible' : 'undo.empty'))
      return
    }
    undoBusy.current = true
    void applyUndo(entry)
      .then((ok) => {
        if (!ok) return
        // The stack can grow or be trimmed while the reversal is in flight; removing by
        // identity keeps a stale pop from dropping the wrong entry.
        const at = undoStack.current.lastIndexOf(entry)
        if (at < 0) return
        undoStack.current.splice(at, 1)
        redoStack.current.push(entry)
        while (redoStack.current.length > undoLimit()) redoStack.current.shift()
        setHistoryEpoch((epoch) => epoch + 1)
      })
      .finally(() => {
        undoBusy.current = false
      })
  }, [applyUndo, showToast, t, undoLimit])

  const redoLast = useCallback(() => {
    if (undoBusy.current) return
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
    undoBusy.current = true
    void runStep(request.kind, request.sources, {
      targetDir: request.targetDir,
      newName: request.newName,
      newNames: request.newNames,
      overwrite: request.overwrite,
      after: afterDirs,
    })
      .then((ok: boolean) => {
        if (!ok) return
        const at = redoStack.current.lastIndexOf(entry)
        if (at < 0) return
        redoStack.current.splice(at, 1)
        undoStack.current.push(entry)
        while (undoStack.current.length > undoLimit()) undoStack.current.shift()
        setHistoryEpoch((epoch) => epoch + 1)
      })
      .finally(() => {
        undoBusy.current = false
      })
  }, [invalidateGit, invalidateTree, runStep, showToast, t, undoLimit])

  const undoTo = useCallback(
    async (targetId: number) => {
      if (undoBusy.current) return
      const index = undoStack.current.findIndex((entry) => entry.id === targetId)
      if (index === -1) return
      undoBusy.current = true
      try {
        // Undo newest-first down to the requested entry. `targetId` is re-resolved every
        // round: the stack may have been trimmed while the previous reversal was in flight.
        for (;;) {
          if (!undoStack.current.some((entry) => entry.id === targetId)) break
          const entry = undoStack.current[undoStack.current.length - 1]
          if (!entry) break
          const ok = await applyUndo(entry)
          if (!ok) break
          const current = undoStack.current.lastIndexOf(entry)
          if (current >= 0) undoStack.current.splice(current, 1)
          redoStack.current.push(entry)
          while (redoStack.current.length > undoLimit()) redoStack.current.shift()
        }
        setHistoryEpoch((epoch) => epoch + 1)
      } finally {
        undoBusy.current = false
      }
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
