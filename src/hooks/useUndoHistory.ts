import { useCallback, useRef, useState } from 'react'
import type { MutableRefObject } from 'react'
import type { Settings } from '../persist'
import type { OpKind } from '../types'
import { errorKey } from '../i18n'
import type { Translate } from '../i18n'
import { pathsExist, restoreRecycle } from '../api'
import { errorCode } from '../errorCode'
import { baseName, joinPath, parentPath, pathKey } from '../paths'
import { UNDO_LIMIT, undoEntryFor } from '../undo'
import type { OpRequestShape, UndoEntry, UndoPayload } from '../undo'

/**
 * Options accepted by the operation runner. Declared structurally (instead of importing
 * `RunOpOptions`) so this module stays independent of `useFileOperations`, which imports
 * this hook.
 */
type StepOptions = {
  targetDir?: string
  newName?: string
  newNames?: string[]
  permanent?: boolean
  overwrite?: boolean
  confirmedProtected?: string[]
  onLanded?: (landed: ReadonlyMap<string, string>) => void
  after?: () => void
}

type RunStep = (kind: OpKind, sources: string[], options?: StepOptions) => Promise<boolean>

export interface UseUndoHistoryOptions {
  settingsRef: MutableRefObject<Settings>
}

/**
 * Undo/redo stacks and the operation bookkeeping that feeds them.
 *
 * Separated from the replay logic because `runOp` must push entries before the history
 * view exists, while replaying needs `runStep` (built on top of `runOp`).
 */
export function useUndoHistory({ settingsRef }: UseUndoHistoryOptions) {
  const undoStack = useRef<UndoEntry[]>([])
  const redoStack = useRef<UndoEntry[]>([])
  const undoCounter = useRef(0)
  /** Incremented whenever a new operation clears redo; in-flight undos compare it. */
  const historyGen = useRef(0)
  const lastIrreversible = useRef(false)
  const [historyEpoch, setHistoryEpoch] = useState(0)

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
      // A new operation invalidates the redo history. An undo that was already in flight
      // must notice this, otherwise it pushes the now-obsolete entry back into redo.
      historyGen.current += 1
      setHistoryEpoch((epoch) => epoch + 1)
    },
    [undoLimit],
  )

  return {
    undoStack,
    redoStack,
    historyGen,
    lastIrreversible,
    historyEpoch,
    setHistoryEpoch,
    undoLimit,
    pushUndo,
  }
}

export interface UseUndoReplayOptions {
  undoStack: MutableRefObject<UndoEntry[]>
  redoStack: MutableRefObject<UndoEntry[]>
  historyGen: MutableRefObject<number>
  /** True after a permanent delete, so the empty-history message says why. */
  lastIrreversible: MutableRefObject<boolean>
  undoLimit: () => number
  setHistoryEpoch: React.Dispatch<React.SetStateAction<number>>
  t: Translate
  showToast: (msg: string, timeout?: number) => void
  refreshPanes: () => void
  invalidateTree: (dir: string) => void
  invalidateGit: (dir: string) => void
  runStep: RunStep
}

/** Reverses and replays history entries; `useFileOperations` builds these from the stacks. */
export function useUndoReplay({
  undoStack,
  redoStack,
  historyGen,
  lastIrreversible,
  undoLimit,
  setHistoryEpoch,
  t,
  showToast,
  refreshPanes,
  invalidateTree,
  invalidateGit,
  runStep,
}: UseUndoReplayOptions) {
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
          // The reverse move itself can land under a different name: if the original name
          // is occupied again, the shell resolves it as "keep both" and creates
          // `name (1).ext` in the folder. The rename below must target that actual path;
          // using the old landed name could rename an unrelated same-named file.
          const landings = new Map<string, string>()
          const moved = await Promise.all(
            [...byDir].map(([dir, paths]) =>
              runStep('move', paths, {
                targetDir: dir,
                after: afterDirs([dir]),
                onLanded: (map) => {
                  for (const [key, value] of map) landings.set(key, value)
                },
              }),
            ),
          )
          if (!moved.every(Boolean)) return false
          // A conflict resolved by the shell as "keep both" left the file under
          // `name (1).ext`. Undo must restore the name it had, or redo (which replays the
          // original `from` path) cannot find it.
          const renames = new Map<string, { paths: string[]; names: string[] }>()
          for (const item of pending) {
            const original = baseName(item.from)
            const actual = landings.get(pathKey(item.to))
            if (!actual || baseName(actual) === original) continue
            const dir = parentPath(actual)
            const group = renames.get(dir) ?? { paths: [], names: [] }
            group.paths.push(actual)
            group.names.push(original)
            renames.set(dir, group)
          }
          const restored = await Promise.all(
            [...renames].map(([dir, group]) =>
              runStep('rename', group.paths, {
                newNames: group.names,
                after: afterDirs([dir]),
              }),
            ),
          )
          // A failed rename (the original name is occupied again) still leaves the files
          // back in their folder, so the undo succeeded. Re-arm the redo entry with the
          // paths that exist now so Ctrl+Y never replays one that is gone.
          const candidates = entry.items.flatMap((item) => {
            const dir = parentPath(item.from)
            const actual =
              landings.get(pathKey(item.to)) ?? joinPath(dir, baseName(item.to))
            return [joinPath(dir, baseName(item.from)), actual]
          })
          const exists = await pathsExist(candidates)
          entry.request = {
            ...entry.request,
            sources: entry.items.map((item, index) =>
              exists[index * 2]
                ? candidates[index * 2]
                : exists[index * 2 + 1]
                  ? candidates[index * 2 + 1]
                  : item.from,
            ),
          }
          if (!restored.every(Boolean)) {
            const failed = [...renames.values()].reduce((sum, group) => sum + group.paths.length, 0)
            showToast(t('undo.partial', { ok: pending.length - failed, failed }))
          }
          return true
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
    // A new operation during the (asynchronous) reversal cleared the redo stack; its
    // completion must not push the obsolete entry back.
    const gen = historyGen.current
    void applyUndo(entry)
      .then((ok) => {
        if (!ok) return
        // The stack can grow or be trimmed while the reversal is in flight; removing by
        // identity keeps a stale pop from dropping the wrong entry.
        const at = undoStack.current.lastIndexOf(entry)
        if (at < 0) return
        undoStack.current.splice(at, 1)
        if (historyGen.current !== gen) {
          setHistoryEpoch((epoch) => epoch + 1)
          return
        }
        redoStack.current.push(entry)
        while (redoStack.current.length > undoLimit()) redoStack.current.shift()
        setHistoryEpoch((epoch) => epoch + 1)
      })
      .catch((err: unknown) => {
        // `applyUndo` awaits IPC (`pathsExist`); a rejection would otherwise leave the
        // user with no feedback and the entry silently stuck on the stack.
        showToast(t('op.failed', { code: t(errorKey(errorCode(err))) }))
      })
      .finally(() => {
        undoBusy.current = false
      })
  }, [applyUndo, historyGen, lastIrreversible, redoStack, setHistoryEpoch, showToast, t, undoLimit, undoStack])

  const redoLast = useCallback(() => {
    if (undoBusy.current) return
    const entry = redoStack.current[redoStack.current.length - 1]
    if (!entry) {
      showToast(t('redo.empty'))
      return
    }
    const { request } = entry
    // Redo used to invalidate only `entry.dirs`, which no caller ever fills. Derive the
    // affected folders from the replayed request instead, so the tree/git badges refresh
    // even when the target is not an open pane.
    const dirs = new Set<string>(entry.dirs)
    for (const source of request.sources) dirs.add(parentPath(source))
    if (request.targetDir) dirs.add(request.targetDir)
    const afterDirs = () => {
      for (const dir of dirs) {
        invalidateTree(dir)
        invalidateGit(dir)
      }
    }
    let landed: ReadonlyMap<string, string> = new Map()
    undoBusy.current = true
    // Same guard as undo: a new operation during the replay has already reset the stacks.
    const gen = historyGen.current
    void runStep(request.kind, request.sources, {
      targetDir: request.targetDir,
      newName: request.newName,
      newNames: request.newNames,
      permanent: request.permanent,
      overwrite: request.overwrite,
      confirmedProtected: request.confirmedProtected,
      after: afterDirs,
      onLanded: (map) => {
        landed = map
      },
    })
      .then((ok: boolean) => {
        if (!ok) return
        const at = redoStack.current.lastIndexOf(entry)
        if (at < 0) return
        redoStack.current.splice(at, 1)
        if (historyGen.current !== gen) {
          setHistoryEpoch((epoch) => epoch + 1)
          return
        }
        // The replay may have landed under a different name ("keep both"). Re-arm the
        // entry with what actually landed so the next Ctrl+Z never targets a path the
        // redo did not create.
        const updated = undoEntryFor(request.kind, request.sources, {
          targetDir: request.targetDir,
          newName: request.newName,
          newNames: request.newNames,
          permanent: request.permanent,
          landed,
        })
        undoStack.current.push(updated ? { ...entry, ...updated } : entry)
        while (undoStack.current.length > undoLimit()) undoStack.current.shift()
        setHistoryEpoch((epoch) => epoch + 1)
      })
      .finally(() => {
        undoBusy.current = false
      })
  }, [historyGen, invalidateGit, invalidateTree, redoStack, runStep, setHistoryEpoch, showToast, t, undoLimit, undoStack])

  const undoTo = useCallback(
    async (targetId: number) => {
      if (undoBusy.current) return
      const index = undoStack.current.findIndex((entry) => entry.id === targetId)
      if (index === -1) return
      undoBusy.current = true
      const gen = historyGen.current
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
          // A new operation reset the history mid-loop; replaying older entries from
          // here would reinstate an order the user already replaced.
          if (historyGen.current !== gen) break
          redoStack.current.push(entry)
          while (redoStack.current.length > undoLimit()) redoStack.current.shift()
        }
        setHistoryEpoch((epoch) => epoch + 1)
      } catch (err: unknown) {
        showToast(t('op.failed', { code: t(errorKey(errorCode(err))) }))
      } finally {
        undoBusy.current = false
      }
    },
    [applyUndo, historyGen, redoStack, setHistoryEpoch, showToast, t, undoLimit, undoStack],
  )

  return { undoLast, redoLast, undoTo }
}
