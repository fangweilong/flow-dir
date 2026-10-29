import type { GitRepo, GitStatus } from './types'

/** Entry-name lookups (lowercase) derived from one folder's status payload. */
export type GitBadges = {
  /** Entries of the listed folder that are changed or untracked. */
  changed: Map<string, string>
  /** Child folders that contain at least one change somewhere below them. */
  dirtyDirs: Set<string>
  /** Tracked child folders with no change; they get the same check as a clean repository. */
  cleanDirs: Set<string>
  /** Tracked child files with no change. */
  cleanFiles: Set<string>
  /** Child repository summaries, shown as a badge on each repository folder icon. */
  repos: Map<string, GitRepo>
}

const NO_CHANGES = new Map<string, string>()
const NO_DIRS = new Set<string>()
const NO_CLEAN = new Set<string>()
const NO_CLEAN_FILES = new Set<string>()
const NO_REPOS = new Map<string, GitRepo>()

/**
 * The backend already pruned the status to the listed folder and collapsed deeper paths
 * into one marker per child folder, so both maps hold at most one entry per visible row.
 * Repository rows come from their own probe and remain valid outside a work tree.
 */
export function buildGitBadges(status: GitStatus | null): GitBadges {
  if (!status) return { changed: NO_CHANGES, dirtyDirs: NO_DIRS, cleanDirs: NO_CLEAN, cleanFiles: NO_CLEAN_FILES, repos: NO_REPOS }
  const repos = new Map(status.repos.map((repo) => [repo.name.toLowerCase(), repo]))
  if (!status.root) return { changed: NO_CHANGES, dirtyDirs: NO_DIRS, cleanDirs: NO_CLEAN, cleanFiles: NO_CLEAN_FILES, repos }
  const changed = new Map<string, string>()
  for (const file of status.files) changed.set(file.path.toLowerCase(), file.code)
  const dirtyDirs = new Set<string>()
  for (const dir of status.dirs) dirtyDirs.add(dir.toLowerCase())
  const cleanDirs = new Set<string>()
  for (const dir of status.cleanDirs) {
    const key = dir.toLowerCase()
    // A dirty marker always wins: the two lists must never disagree.
    if (!dirtyDirs.has(key)) cleanDirs.add(key)
  }
  const cleanFiles = new Set<string>()
  for (const file of status.cleanFiles ?? []) {
    const key = file.toLowerCase()
    if (!changed.has(key)) cleanFiles.add(key)
  }
  return { changed, dirtyDirs, cleanDirs, cleanFiles, repos }
}
