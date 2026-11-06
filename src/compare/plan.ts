import type { CompareOptions } from '../api'
import { joinPath } from '../paths'
import type { CompareEntry, CompareStatus } from '../types'

/** One confirmed step of the preview; the app executes these through the normal op pipeline. */
export type SyncAction =
  | { kind: 'copy'; source: string; targetDir: string }
  | { kind: 'delete'; paths: string[] }

export type SyncPlan = {
  actions: SyncAction[]
  conflicts: number
  mismatches: number
}

export type SyncStrategy = 'l2r' | 'r2l' | 'both'
export type CompareDecision = SyncAction | 'conflict' | 'mismatch' | null

/** Filter switch order is the table's column order; keep it stable for the status chips. */
export const STATUS_ORDER: CompareStatus[] = ['left', 'right', 'different', 'same']

/** Inputs are megabytes/days; the backend wants bytes/milliseconds. */
export function parseMb(value: string): number {
  const parsed = Number(value)
  return Number.isFinite(parsed) && parsed > 0 ? Math.round(parsed * 1024 * 1024) : 0
}

export function splitList(value: string): string[] {
  return value
    .split(/[\s,;]+/)
    .map((item) => item.trim())
    .filter(Boolean)
}

export function parentRel(rel: string): string {
  const cut = rel.lastIndexOf('\\')
  return cut < 0 ? '' : rel.slice(0, cut)
}

// Excel evaluates a quoted cell that starts with `=`, `+`, `-` or `@` as a formula, so a
// file name like `=cmd|'/c calc'!A1` would run when the exported CSV is opened. The
// leading apostrophe keeps such values literal.
export function csvCell(value: string | number): string {
  const text = String(value)
  const safe = /^[=+\-@\t\r]/.test(text) ? `'${text}` : text
  return `"${safe.replace(/"/g, '""')}"`
}

export type CompareFilterInput = {
  includeHidden: boolean
  extensions: string
  minMb: string
  maxMb: string
  days: string
  exclude: string
  toleranceSec: string
  /** Injectable clock for tests; production uses the current time. */
  now?: number
}

export function buildCompareOptions(input: CompareFilterInput): CompareOptions {
  const days = Number(input.days)
  return {
    includeHidden: input.includeHidden,
    extensions: splitList(input.extensions).map((ext) => ext.replace(/^\./, '').toLowerCase()),
    minSize: parseMb(input.minMb),
    maxSize: parseMb(input.maxMb),
    afterMs: Number.isFinite(days) && days > 0 ? (input.now ?? Date.now()) - days * 86_400_000 : 0,
    beforeMs: 0,
    exclude: splitList(input.exclude),
    toleranceMs: Math.max(0, Math.round((Number(input.toleranceSec) || 0) * 1000)),
  }
}

export type DecideInput = {
  entry: CompareEntry
  leftRoot: string
  rightRoot: string
  strategy: SyncStrategy
  deleteExtras: boolean
  /** The pair failed hash verification, so neither side is safe to overwrite. */
  hashError: boolean
  toleranceMs: number
}

/**
 * Maps one row to the action a sync would take. `conflict` and `mismatch` are reported
 * separately because they are never executed automatically: a bidirectional sync with
 * equal timestamps cannot choose a direction, and a file/folder pair has no sane merge.
 */
export function decideAction({
  entry,
  leftRoot,
  rightRoot,
  strategy,
  deleteExtras,
  hashError,
  toleranceMs,
}: DecideInput): CompareDecision {
  const leftPath = joinPath(leftRoot, entry.rel)
  const rightPath = joinPath(rightRoot, entry.rel)
  const targetLeft = joinPath(leftRoot, parentRel(entry.rel))
  const targetRight = joinPath(rightRoot, parentRel(entry.rel))

  if (entry.left?.isDir || entry.right?.isDir) {
    if (entry.left && entry.right) {
      return entry.left.isDir !== entry.right.isDir ? 'mismatch' : null
    }
    if (entry.left && !entry.right) {
      if (strategy === 'r2l') return deleteExtras ? { kind: 'delete', paths: [leftPath] } : null
      return { kind: 'copy', source: leftPath, targetDir: targetRight }
    }
    if (entry.right && !entry.left) {
      if (strategy === 'l2r') return deleteExtras ? { kind: 'delete', paths: [rightPath] } : null
      return { kind: 'copy', source: rightPath, targetDir: targetLeft }
    }
    return 'mismatch'
  }

  if (entry.left && entry.right) {
    if (entry.status === 'same') return null
    // Verification failed on one side: the copy would overwrite a file we could not
    // read, so keep the pair out of the plan.
    if (hashError) return null
    // A side the filters hide is only shown because the other side matched; copying
    // from it would propagate a file the user scoped out.
    const leftUsable = entry.leftCompared !== false
    const rightUsable = entry.rightCompared !== false
    if (strategy === 'l2r') {
      return leftUsable ? { kind: 'copy', source: leftPath, targetDir: targetRight } : null
    }
    if (strategy === 'r2l') {
      return rightUsable ? { kind: 'copy', source: rightPath, targetDir: targetLeft } : null
    }
    const diff = entry.left.mtime - entry.right.mtime
    if (Math.abs(diff) <= toleranceMs) return 'conflict'
    if (diff > 0) {
      return leftUsable ? { kind: 'copy', source: leftPath, targetDir: targetRight } : null
    }
    return rightUsable ? { kind: 'copy', source: rightPath, targetDir: targetLeft } : null
  }

  if (entry.left) {
    if (strategy === 'r2l') return deleteExtras ? { kind: 'delete', paths: [leftPath] } : null
    return { kind: 'copy', source: leftPath, targetDir: targetRight }
  }
  if (strategy === 'l2r') return deleteExtras ? { kind: 'delete', paths: [rightPath] } : null
  return { kind: 'copy', source: rightPath, targetDir: targetLeft }
}
