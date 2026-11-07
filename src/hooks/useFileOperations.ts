import { useCallback, useRef, useState } from 'react'
import type { MutableRefObject } from 'react'
import type { LandedItem, OpKind, OpProgress } from '../types'
import type { Settings } from '../persist'
import { errorKey } from '../i18n'
import type { Translate } from '../i18n'
import type { SyncAction } from '../ComparePanel'
import { notifyOp, opRetryElevated, opStart, pathsExist, restoreRecycle } from '../api'
import { errorCode } from '../errorCode'
import { joinPath, parentPath } from '../paths'
import { protectionError, protectWord } from '../protection'
import type { ProtectPromptState } from '../protection'
import { undoEntryFor, UNDO_LIMIT } from '../undo'
import type { OpRequestShape, UndoEntry, UndoPayload } from '../undo'
import { landedIndices } from '../undoLanded'
import { useArchiveJobs } from './useArchiveJobs'
import { useTaskbarProgress } from './useTaskbarProgress'

// Re-exported for ModalContainer, which reports operation failures with the same mapping.
export { errorCode } from '../errorCode'

function uid(prefix: string): string {
  return `${prefix}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`
}

// The shell reports the item it actually created; a "keep both" conflict lands as
// `name (1).ext`, so undo must target that path rather than the requested name.
function landedMap(items: LandedItem[] | undefined, list: readonly string[]) {
  const map = new Map<string, string>()
  for (const item of items ?? []) {
    const source = list[item.index]
    if (source) map.set(source.toLowerCase(), item.path)
  }
  return map
}

export type RunOpOptions = {
  targetDir?: string
  newName?: string
  newNames?: string[]
  permanent?: boolean
  overwrite?: boolean
  /** Paths the user already confirmed for the `protect_confirm` tier. */
  confirmedProtected?: string[]
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
  // Prompts are queued: two protected operations can fail in the same tick (a pane move
  // and a drop from Explorer), and a single state slot would replace the first prompt's
  // confirm/cancel callback, leaving that operation's promise unresolved forever.
  const [protectPrompt, setCurrentProtectPrompt] = useState<ProtectPromptState | null>(null)
  const protectQueue = useRef<ProtectPromptState[]>([])
  const setProtectPrompt = useCallback((prompt: ProtectPromptState | null) => {
    const queue = protectQueue.current
    if (prompt) {
      queue.push(prompt)
      if (queue.length === 1) setCurrentProtectPrompt(prompt)
      return
    }
    queue.shift()
    setCurrentProtectPrompt(queue[0] ?? null)
  }, [])

  const undoStack = useRef<UndoEntry[]>([])
  const redoStack = useRef<UndoEntry[]>([])
  const undoCounter = useRef(0)
  const lastIrreversible = useRef(false)
  const opRequests = useRef<Record<string, OpRequestShape & { opId: string; record?: boolean }>>({})

  const archiveJobs = useArchiveJobs({ setOps, refreshPanes, showToast, t })
  useTaskbarProgress(ops)

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
        confirmedProtected: options?.confirmedProtected,
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
      const launch = (confirmedProtected?: string[]) => {
        const pending = opRequests.current[opId]
        // A rule change between the prompt and the retry can surface a second path; keep
        // the earlier confirmations instead of asking the same question twice.
        const merged = confirmedProtected
          ? [...new Set([...(pending?.confirmedProtected ?? []), ...confirmedProtected])]
          : undefined
        if (pending && merged) pending.confirmedProtected = merged
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
            confirmedProtected: merged,
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
                    landed: landedMap(progress.landed, sources),
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
              const keep = landedIndices(sources, progress)
              const landed = new Set(keep)
              recordEntry(
                undoEntryFor(kind, sources, {
                  ...options,
                  skip: sources.filter((_, index) => !landed.has(index)),
                  landed: landedMap(progress.landed, sources),
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
            recordEntry(
              undoEntryFor(kind, sources, {
                ...options,
                landed: landedMap(progress.landed, sources),
              }),
              options?.dirsAfter ?? [],
            )
            options?.after?.()
            refreshPanes()
            remind(t('op.done'))
            options?.onDone?.(true)
            drop(800)
          },
        ).catch((e: unknown) => {
          const guard = protectionError(e)
          if (guard) {
            const blocked = guard.code === 'PROTECTED'
            setProtectPrompt({
              mode: blocked ? 'block' : 'confirm',
              paths: guard.paths,
              word: blocked ? '' : protectWord(kind),
              onConfirm: () => {
                setProtectPrompt(null)
                if (!blocked) launch(guard.paths)
              },
              onCancel: () => {
                setProtectPrompt(null)
                options?.onDone?.(false)
                if (!blocked) drop(0)
              },
            })
            if (blocked) {
              options?.onDone?.(false)
              drop(0)
            }
            return
          }
          showToast(t('op.failed', { code: t(errorKey(errorCode(e))) }))
          options?.onDone?.(false)
          drop(4000)
        })
      }
      launch(options?.confirmedProtected)
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
          const keep = landedIndices(retry.sources, {
            succeeded: report.succeeded ?? [],
            failedIndices: report.failedIndices ?? [],
            failed: report.failed,
            failedCount: report.failedTotal,
            aborted: report.aborted,
          })
          const landings = landedMap(report.landed, retry.sources)
          // A capped landing report cannot say which path a tail item got. Recording the
          // requested name would make undo delete a pre-existing file on a "keep both"
          // conflict, so those items are left out of the history entry.
          const recordable = report.landedTruncated
            ? keep.filter((index) => landings.has(retry.sources[index].toLowerCase()))
            : keep
          const completed = recordable.map((index) => retry.sources[index])
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
            const recorded = new Set(recordable)
            const entry = undoEntryFor(retry.kind, retry.sources, {
              targetDir: retry.targetDir,
              newName: retry.newName,
              newNames: retry.newNames,
              permanent: retry.permanent,
              skip: retry.sources.filter((_, index) => !recorded.has(index)),
              landed: landings,
            })
            if (entry) {
              pushUndo(
                entry,
                {
                  ...retry,
                  sources: completed,
                  newNames: retry.newNames
                    ? recordable.map((index) => retry.newNames![index])
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
          runOp('copy', sources, {
            targetDir,
            overwrite: true,
            // A failed copy must stop the plan before the delete step: removing the
            // "extra" files after an incomplete copy leaves the destination worse than
            // either half of the sync alone.
            onDone: (ok) => (ok ? runNext() : resolve()),
          }),
        )
      }
      if (deletes.length) {
        steps.push(() => runOp('delete', deletes, { onDone: () => resolve() }))
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
      confirmedProtected: request.confirmedProtected,
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

  return {
    ops,
    setOps,
    elevating,
    runOp,
    retryElevated,
    runSyncActions,
    undoLast,
    redoLast,
    undoTo,
    undoStack,
    redoStack,
    historyEpoch,
    protectPrompt,
    setProtectPrompt,
    ...archiveJobs,
  }
}
