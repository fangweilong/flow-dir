import { pathKey } from './paths'
import type { OpFailure } from './types'

/** Fields shared by an operation progress event and the elevated retry report. */
export type LandingReport = {
  succeeded?: number[]
  failedIndices?: number[]
  failed: readonly OpFailure[]
  failedCount: number
  aborted?: boolean
}

/**
 * Source indices that reached disk, used to record a partial batch for undo/redo.
 *
 * The shell's per-item list (`failed`) is capped, so it cannot identify the landed items
 * of a large batch. `succeeded`/`failedIndices` are authoritative and take precedence;
 * the path list is only trusted when it is complete, and a batch-level failure (empty
 * path) never counts as a landed match.
 */
export function landedIndices(
  sources: readonly string[],
  report: LandingReport,
): number[] {
  const succeeded = report.succeeded ?? []
  // A cancellation reports what finished; there is nothing left to infer from an
  // incomplete failure list, and guessing would record items that never landed.
  if (succeeded.length || report.aborted) return succeeded
  const all = sources.map((_, index) => index)
  if (report.failedIndices?.length) {
    const failed = new Set(report.failedIndices)
    return all.filter((index) => !failed.has(index))
  }
  if (report.failedCount === report.failed.length && report.failed.every((failure) => failure.path)) {
    const failed = new Set(report.failed.map((failure) => pathKey(failure.path)))
    return all.filter((index) => !failed.has(pathKey(sources[index])))
  }
  return []
}
