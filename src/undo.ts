import { baseName, joinPath } from './paths'
import type { OpKind } from './types'

export type RenameMove = { path: string; oldName: string; newName: string }

/** What an entry reverses; the metadata below stays attached when it moves to the redo stack. */
export type UndoPayload =
  | { kind: 'rename'; items: RenameMove[] }
  | { kind: 'move'; items: { from: string; to: string }[] }
  | { kind: 'copy'; created: string[] }
  | { kind: 'delete'; paths: string[] }

/** The request that produced the entry, replayed verbatim by redo. */
export type OpRequestShape = {
  kind: OpKind
  sources: string[]
  targetDir?: string
  newName?: string
  newNames?: string[]
  permanent?: boolean
}

export type UndoEntry = UndoPayload & {
  id: number
  /** Unix milliseconds; shown in the history panel. */
  at: number
  request: OpRequestShape
  /** Folders whose listing/tree needs invalidating after the entry is reversed. */
  dirs: string[]
}

/** Default depth when `settings.undo_limit` is missing; both stacks follow that setting. */
export const UNDO_LIMIT = 10

/**
 * Builds the reverse information for a successful operation. Permanent deletes and folder
 * creation have no safe inverse, so they return null and never enter the stack.
 *
 * `skip` lists sources the shell refused: a partly successful batch still records what did
 * land on disk, so Ctrl+Z can undo the part that worked instead of skipping past it.
 */
export function undoEntryFor(
  kind: OpKind,
  sources: string[],
  options?: {
    targetDir?: string
    newName?: string
    newNames?: string[]
    permanent?: boolean
    skip?: string[]
  },
): UndoPayload | null {
  if (!sources.length) return null
  const skip = new Set((options?.skip ?? []).map((path) => path.toLowerCase()))
  const kept = sources
    .map((path, index) => ({ path, index }))
    .filter((item) => !skip.has(item.path.toLowerCase()))
  if (!kept.length) return null
  const paths = kept.map((item) => item.path)
  if (kind === 'rename') {
    if (options?.newNames && options.newNames.length === sources.length) {
      return {
        kind: 'rename',
        items: kept.map(({ path, index }) => ({
          path,
          oldName: baseName(path),
          newName: options.newNames![index],
        })),
      }
    }
    if (options?.newName && paths.length === 1) {
      return {
        kind: 'rename',
        items: [{ path: paths[0], oldName: baseName(paths[0]), newName: options.newName }],
      }
    }
  }
  if (kind === 'move' && options?.targetDir) {
    const target = options.targetDir
    return { kind: 'move', items: paths.map((from) => ({ from, to: joinPath(target, baseName(from)) })) }
  }
  if (kind === 'copy' && options?.targetDir) {
    const target = options.targetDir
    return { kind: 'copy', created: paths.map((from) => joinPath(target, baseName(from))) }
  }
  if (kind === 'delete' && !options?.permanent) {
    return { kind: 'delete', paths }
  }
  return null
}
