import type { GitStatus } from './types'

/** Entry-name lookups (lowercase) derived from one folder's status payload. */
export type GitBadges = {
  /** Entries of the listed folder that are changed or untracked. */
  changed: Map<string, string>
  /** Child folders that contain at least one change somewhere below them. */
  dirtyDirs: Set<string>
  /** Child folders that are repository roots, marked even when they are clean. */
  repos: Set<string>
}

const NO_CHANGES = new Map<string, string>()
const NO_DIRS = new Set<string>()

/**
 * The backend already pruned the status to the listed folder and collapsed deeper paths
 * into one marker per child folder, so both maps hold at most one entry per visible row.
 * Repository rows come from a membership probe and remain valid outside a work tree.
 */
export function buildGitBadges(status: GitStatus | null): GitBadges {
  if (!status) return { changed: NO_CHANGES, dirtyDirs: NO_DIRS, repos: new Set() }
  const repos = new Set(status.repos.map((name) => name.toLowerCase()))
  if (!status.root) return { changed: NO_CHANGES, dirtyDirs: NO_DIRS, repos }
  const changed = new Map<string, string>()
  for (const file of status.files) changed.set(file.path.toLowerCase(), file.code)
  const dirtyDirs = new Set<string>()
  for (const dir of status.dirs) dirtyDirs.add(dir.toLowerCase())
  return { changed, dirtyDirs, repos }
}
