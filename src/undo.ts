import { baseName, joinPath } from './paths'
import type { OpKind } from './types'

/** One reversible operation; the stack is process-local and newest last. */
export type UndoEntry =
  | { kind: 'rename'; path: string; oldName: string; newName: string }
  | { kind: 'move'; items: { from: string; to: string }[] }
  | { kind: 'copy'; created: string[] }
  | { kind: 'delete'; paths: string[] }

/** Ten steps is enough to recover a mistake without becoming a hidden history. */
export const UNDO_LIMIT = 10

/**
 * Builds the reverse information for a fully successful operation. Permanent deletes and
 * folder creation have no safe inverse, so they return null and never enter the stack.
 */
export function undoEntryFor(
  kind: OpKind,
  sources: string[],
  options?: { targetDir?: string; newName?: string; permanent?: boolean },
): UndoEntry | null {
  if (!sources.length) return null
  if (kind === 'rename' && options?.newName && sources.length === 1) {
    return { kind: 'rename', path: sources[0], oldName: baseName(sources[0]), newName: options.newName }
  }
  if (kind === 'move' && options?.targetDir) {
    const target = options.targetDir
    return { kind: 'move', items: sources.map((from) => ({ from, to: joinPath(target, baseName(from)) })) }
  }
  if (kind === 'copy' && options?.targetDir) {
    const target = options.targetDir
    return { kind: 'copy', created: sources.map((from) => joinPath(target, baseName(from))) }
  }
  if (kind === 'delete' && !options?.permanent) {
    return { kind: 'delete', paths: sources }
  }
  return null
}
