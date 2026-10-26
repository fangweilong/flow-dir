import type { GitStatus } from './types'

/** Entry-name lookups (lowercase) derived from one folder's status payload. */
export type GitBadges = {
  /** Entries of the listed folder that are changed or untracked. */
  changed: Map<string, string>
  /** Child folders that contain at least one change somewhere below them. */
  dirtyDirs: Set<string>
}

const EMPTY: GitBadges = { changed: new Map(), dirtyDirs: new Set() }

/**
 * The backend already pruned the status to the listed folder and collapsed deeper paths
 * into one marker per child folder, so both maps hold at most one entry per visible row.
 */
export function buildGitBadges(status: GitStatus | null): GitBadges {
  if (!status?.root) return EMPTY
  const changed = new Map<string, string>()
  for (const file of status.files) changed.set(file.path.toLowerCase(), file.code)
  const dirtyDirs = new Set<string>()
  for (const dir of status.dirs) dirtyDirs.add(dir.toLowerCase())
  return { changed, dirtyDirs }
}
