export type ApiError = { code: string; detail?: string }
export type RawEntry = [string, number, number, number, number]
export type DirBatch = {
  gen: number
  scanned: number
  done: boolean
  /** The entry cap cut the listing short; only meaningful on the final batch. */
  truncated: boolean
  items: RawEntry[]
  error: ApiError | null
}
export type Drive = {
  root: string
  kind: string
  name: string
  /** Volume label; empty for unlabeled volumes and network locations. */
  label: string
  /** Filesystem name (`NTFS`, `exFAT`, …); empty when the volume cannot be queried. */
  fs: string
  /** Total and free bytes; 0 when unknown (locations, empty optical drives). */
  total: number
  free: number
  /** False when the media cannot be read now (empty optical drive, disconnected share). */
  available: boolean
}
export type Place = { key: string; path: string }
export type Lang = 'zh-CN' | 'en'
/** Theme ids; labels and schemes live in `themes.ts`, palettes in `styles.css`. */
export type Theme =
  | 'system'
  | 'light'
  | 'sepia'
  | 'mist'
  | 'dark'
  | 'midnight'
  | 'graphite'
  | 'nord'
  | 'forest'
export type Dependency = { name: string; version: string; license: string; kind: string }
export type Clip = { paths: string[]; cut: boolean }
export type EverythingStatus = { usable: boolean; version: string | null; reason: string | null }
/** `action` is FlowDir's own appended entry (e.g. the hidden-items toggle), not a shell verb. */
export type MenuOutcome = {
  invoked: boolean
  id: number
  action: string | null
  /** Archive format of a `compress` action. */
  format?: string | null
}
export type OpKind = 'copy' | 'move' | 'delete' | 'rename' | 'newdir'
/** One item the shell rejected; `code` is the mapped app error code. */
export type OpFailure = { path: string; code: string; detail: string }
export type OpProgress = {
  opId: string
  phase: string
  current: number
  total: number
  name: string | null
  work: number
  workTotal: number
  /** Live count of failed items; `failed` only arrives with the final message. */
  failedCount: number
  failed: OpFailure[]
  done: boolean
  aborted: boolean
  error: ApiError | null
}
/** Outcome written by the elevated helper for a retried, previously denied operation. */
export type HashResult = {
  path: string
  algorithm: string
  hash: string
  size: number
  error: string | null
}
export type HashProgress = {
  jobId: string
  phase: string
  current: number
  total: number
  name: string | null
  bytes: number
  bytesTotal: number
  paused: boolean
  done: boolean
  aborted: boolean
  error: ApiError | null
  results: HashResult[]
}
export type DupEntry = { path: string; size: number; mtime: number }
export type DupGroup = { size: number; hash: string; files: DupEntry[] }
export type DupProgress = {
  jobId: string
  phase: string
  current: number
  total: number
  name: string | null
  bytes: number
  bytesTotal: number
  paused: boolean
  done: boolean
  aborted: boolean
  error: ApiError | null
  groups: DupGroup[]
  scanned: number
  skippedHidden: number
  skippedCloud: number
  skippedHardlink: number
  elapsedMs: number
}
export type ElevatedReport = {
  aborted: boolean
  failed: OpFailure[]
  failedTotal: number
  error: ApiError | null
}
export type AppInfo = { version: string; dependencies: Dependency[] }

export type UpdateStatus = {
  current: string
  latest: string | null
  available: boolean
  flavor: 'portable' | 'installer'
  notes: string | null
  checked: boolean
  lastCheck: number | null
  error: ApiError | null
}

export type UpdateProgress = {
  phase: 'connecting' | 'downloading' | 'verifying' | 'ready'
  downloaded: number
  total: number
  percent: number
  bytesPerSec: number
}

export type Entry = { name: string; size: number; mtime: number; attrs: number; id: number }

export type SortKey = 'name' | 'size' | 'mtime' | 'type'
export type SortDir = 'asc' | 'desc'
export type SortState = { key: SortKey; dir: SortDir }

export type ListData = {
  entries: Entry[]
  loading: boolean
  scanned: number
  error: ApiError | null
  /** The directory exceeded the entry cap; the footer explains the cut. */
  truncated: boolean
  selected: Set<string>
  /** Name of the row Shift-extends from; a name survives sorting and filtering, an index does not. */
  anchor: string
  filter: string
}

export const emptyList = (): ListData => ({
  entries: [],
  loading: true,
  scanned: 0,
  error: null,
  truncated: false,
  selected: new Set(),
  anchor: '',
  filter: '',
})

/** The one filter predicate the list, the selection and the clipboard all agree on. */
export const filterMatches = (name: string, filter: string): boolean => {
  const q = filter.trim().toLowerCase()
  return !q || name.toLowerCase().includes(q)
}

/**
 * Filtering hides rows, so it also drops them from the selection and the Shift anchor.
 * Without this the pane would still delete, rename or copy a row the user cannot see.
 */
export function filterList(data: ListData, filter: string): ListData {
  if (data.filter === filter) return data
  if (!filter.trim()) return { ...data, filter }
  const selected = new Set<string>()
  for (const name of data.selected) if (filterMatches(name, filter)) selected.add(name)
  return {
    ...data,
    filter,
    selected,
    anchor: filterMatches(data.anchor, filter) ? data.anchor : '',
  }
}

export const EMPTY_LIST: ListData = emptyList()

export type ArchiveEntry = { name: string; size: number }
export type MetaRow = { label: string; value: string }
export type PreviewInfo = {
  /** `cloud` marks a placeholder whose bytes are still in the cloud; nothing was read. */
  kind: 'image' | 'pdf' | 'video' | 'text' | 'archive' | 'binary' | 'folder' | 'unsupported' | 'cloud'
  url?: string | null
  text?: string | null
  entries?: ArchiveEntry[] | null
  meta?: MetaRow[] | null
  reason?: string | null
  /** Caption of the configured external preview program for this extension. */
  external?: string | null
  size: number
  mtime: number
}
export type GitFile = { path: string; code: string }
/** Status summary of one direct child repository, shown on its folder icon. */
export type GitRepo = {
  name: string
  /** False when the probe failed or the folder exceeded the probe limit. */
  checked: boolean
  dirty: boolean
  ahead: number
  behind: number
  upstream: boolean
  conflict: boolean
}
/** Work-tree status of one folder; `root` is empty outside a repository. */
export type GitStatus = {
  root: string
  files: GitFile[]
  /** Child folders with changes deeper down. */
  dirs: string[]
  /** Tracked child folders without any change below them. */
  cleanDirs: string[]
  /** Checked-out branch; empty when HEAD is detached. */
  branch: string
  ahead: number
  behind: number
  /** Whether the branch has an upstream, so the counts mean anything. */
  upstream: boolean
  /** Any change anywhere in the work tree, not only below the listed folder. */
  dirty: boolean
  /** Child folders that are repository roots, with their own summary. */
  repos: GitRepo[]
}

/** Not a repository, or a lookup that failed; also the cache entry for both. */
export const EMPTY_GIT: GitStatus = {
  root: '',
  files: [],
  dirs: [],
  cleanDirs: [],
  branch: '',
  ahead: 0,
  behind: 0,
  upstream: false,
  dirty: false,
  repos: [],
}
export type SearchItem = {
  path: string
  name: string
  isDir: boolean
  size: number
  mtime: number
  /** Matching line for content hits; absent for name-only hits. */
  snippet?: string
}
export type SearchBatch = {
  gen: number
  provider: string
  items: SearchItem[]
  done: boolean
  error?: ApiError | null
}

/** One side's metadata in a folder comparison; `size` is 0 for folders. */
export type CompareMeta = { size: number; mtime: number; isDir: boolean }
export type CompareStatus = 'left' | 'right' | 'different' | 'same'
export type CompareEntry = {
  /** Path relative to the two compare roots. */
  rel: string
  name: string
  left: CompareMeta | null
  right: CompareMeta | null
  status: CompareStatus
}
export type CompareLevel = { entries: CompareEntry[] }
/** Recursive file counts for one shared child folder. */
export type CompareCount = {
  name: string
  leftOnly: number
  rightOnly: number
  different: number
  same: number
  scanned: number
  done: boolean
  aborted: boolean
  error: ApiError | null
}
export type CompareHashPair = { rel: string; left: string; right: string }
export type CompareHashResult = {
  rel: string
  equal: boolean
  leftHash: string | null
  rightHash: string | null
  error: string | null
}
export type CompareHashEvent = {
  jobId: string
  current: number
  total: number
  name: string | null
  bytes: number
  bytesTotal: number
  paused: boolean
  done: boolean
  aborted: boolean
  error: ApiError | null
  results: CompareHashResult[]
}

/** Folder (and optional row) to open on launch or from the tray; mirrors the Rust type. */
export type LaunchRequest = {
  path: string | null
  select: string | null
  newWindow: boolean
  autostart: boolean
}
