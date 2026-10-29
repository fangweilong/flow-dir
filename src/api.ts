import { Channel, invoke } from '@tauri-apps/api/core'
import type { AppInfo, Clip, CompareCount, CompareHashEvent, CompareHashPair, CompareLevel, DirBatch, Drive, DupProgress, ElevatedReport, HashProgress, MenuOutcome, OpKind, OpProgress, Place, UpdateStatus } from './types'
import type { GitStatus, LaunchRequest, PreviewInfo, SearchBatch } from './types'
import type { Settings } from './persist'

export function listDir(
  path: string,
  tabId: string,
  gen: number,
  sort: string,
  dir: string,
  hidden: boolean,
  onBatch: (batch: DirBatch) => void,
) {
  const channel = new Channel<DirBatch>()
  channel.onmessage = onBatch
  return invoke<void>('list_dir', { path, tabId, gen, sort, dir, hidden, onBatch: channel })
}

export const cancelDir = (tabId: string) => invoke<void>('cancel_dir', { tabId })
export const drives = () => invoke<Drive[]>('drives')
/** Subdirectory names, for the sidebar tree; `truncated` marks a folder that hit the cap. */
export const treeDirs = (path: string, hidden: boolean) =>
  invoke<{ names: string[]; truncated: boolean }>('tree_dirs', { path, hidden })
/** True when the path is an existing folder; validates an address-bar edit before navigating. */
export const pathIsDir = (path: string) => invoke<boolean>('path_is_dir', { path })
/** Expands environment variables (%VAR%) and ~ in a path. */
export const expandPath = (raw: string) => invoke<string>('expand_path', { raw })
/** Repository status for the folder; empty when it is not a work tree. */
export const gitStatus = (path: string) => invoke<GitStatus>('git_status', { path })
export const quickAccess = () => invoke<Place[]>('quick_access')
export const openPath = (path: string) => invoke<void>('open_path', { path })
/** System "Open with" dialog, for a file with no default program. */
export const openWith = (path: string) => invoke<void>('open_with', { path })
/** Which of the paths already exist on disk; used before a rename overwrites one. */
export const pathsExist = (paths: string[]) => invoke<boolean[]>('paths_exist', { paths })
/** Free `name (n)` variants, in the same order, for conflict-free batch renames. */
export const uniquePaths = (paths: string[]) => invoke<string[]>('unique_paths', { paths })

/** Row shape for {@link sortOrder}; mirrors the fields a listing already carries. */
export type SortItem = { name: string; path?: string; size: number; mtime: number; isDir: boolean }
/**
 * Display order for rows already on screen. Uses the backend's Explorer-compatible
 * comparator, so changing the sort column never re-enumerates the folder.
 */
export const sortOrder = (items: SortItem[], key: string, dir: string) =>
  invoke<number[]>('sort_order', { items, key, dir })
export const revealPath = (path: string) => invoke<void>('reveal_path', { path })
export const explorerOpen = (path: string) => invoke<void>('explorer_open', { path })

export const layoutSave = (workspace: unknown) => invoke<void>('layout_save', { workspace })
export const layoutLoad = () => invoke<unknown>('layout_load')
export const layoutPreserve = () => invoke<string>('layout_preserve')

export const settingsLoad = () => invoke<Settings>('settings_load')
export const configWarnings = () => invoke<string[]>('config_warnings')
/** Returns integration warning codes so the caller can toast a hotkey conflict. */
export const settingsSave = (settings: Settings) => invoke<string[]>('settings_save', { settings })
/** Folder requested by the command line on first launch; consumed once. */
export const takeLaunchRequest = () => invoke<LaunchRequest | null>('take_launch_request')
/** Rebuilds the tray's recent-folder submenu. */
export const trayRecent = (dirs: string[]) => invoke<void>('tray_recent', { dirs })
export const setAppTheme = (theme: string) => invoke<void>('set_app_theme', { theme })
export const watchSet = (paneId: string, path: string) => invoke<void>('watch_set', { paneId, path })
export const watchClear = (paneId: string) => invoke<void>('watch_clear', { paneId })
export const clipboardCopy = (paths: string[], cut: boolean) => invoke<void>('clipboard_copy', { paths, cut })
export const clipboardPaste = () => invoke<Clip>('clipboard_paste')
/** Starts an OLE drag of the paths; resolves to the accepted effect: copy/move/link/none. */
export const dragOut = (paths: string[]) => invoke<string>('drag_out', { paths })

export type OpRequest = {
  opId: string
  kind: OpKind
  sources: string[]
  targetDir?: string
  newName?: string
  /** One name per source for a batch rename; takes precedence over `newName`. */
  newNames?: string[]
  permanent?: boolean
  /** Answer the shell's replace prompt automatically; used by the sync preview. */
  overwrite?: boolean
}

export function opStart(request: OpRequest, onProgress: (progress: OpProgress) => void) {
  const channel = new Channel<OpProgress>()
  channel.onmessage = onProgress
  return invoke<void>('op_start', {
    opId: request.opId,
    kind: request.kind,
    sources: request.sources,
    targetDir: request.targetDir ?? null,
    newName: request.newName ?? null,
    newNames: request.newNames ?? null,
    permanent: request.permanent ?? false,
    overwrite: request.overwrite ?? false,
    onProgress: channel,
  })
}

export const opCancel = (opId: string) => invoke<void>('op_cancel', { opId })

/** Extracts one archive into `dest`; cancel it through {@link opCancel}. */
export function archiveExtract(
  jobId: string,
  archive: string,
  dest: string,
  password: string | null,
  onProgress: (progress: OpProgress) => void,
) {
  const channel = new Channel<OpProgress>()
  channel.onmessage = onProgress
  return invoke<void>('archive_extract', { jobId, archive, dest, password, onProgress: channel })
}

/** Compresses `paths` into `dest`; cancel it through {@link opCancel}. */
export function archiveCompress(
  jobId: string,
  paths: string[],
  dest: string,
  format: CompressFormat,
  level: number,
  password: string | null,
  onProgress: (progress: OpProgress) => void,
) {
  const channel = new Channel<OpProgress>()
  channel.onmessage = onProgress
  return invoke<void>('archive_compress', { jobId, paths, dest, format, level, password, onProgress: channel })
}

/** Resolves archive virtual paths to cached real files so OLE drag can offer them. */
export const archivePrepareDrag = (paths: string[]) =>
  invoke<string[]>('archive_prepare_drag', { paths })

/** Extracts individual virtual entries from an archive into `dest`. */
export const archiveExtractItems = (items: string[], dest: string) =>
  invoke<string[]>('archive_extract_items', { items, dest })

/** Streaming SHA-256/SHA-1/MD5 over the given paths; results arrive with the final message. */
export function hashStart(
  jobId: string,
  paths: string[],
  algorithm: string,
  onProgress: (progress: HashProgress) => void,
) {
  const channel = new Channel<HashProgress>()
  channel.onmessage = onProgress
  return invoke<void>('hash_start', { jobId, paths, algorithm, onProgress: channel })
}

export type DupOptions = {
  roots: string[]
  includeHidden: boolean
  minSize: number
  concurrency: number
  maxBytesPerSec: number
  exclude: string[]
}

export function dupStart(
  jobId: string,
  options: DupOptions,
  onProgress: (progress: DupProgress) => void,
) {
  const channel = new Channel<DupProgress>()
  channel.onmessage = onProgress
  return invoke<void>('dup_start', {
    jobId,
    roots: options.roots,
    includeHidden: options.includeHidden,
    minSize: options.minSize,
    concurrency: options.concurrency,
    maxBytesPerSec: options.maxBytesPerSec,
    exclude: options.exclude,
    onProgress: channel,
  })
}

export const toolCancel = (jobId: string) => invoke<void>('tool_cancel', { jobId })
export const toolPause = (jobId: string, paused: boolean) =>
  invoke<void>('tool_pause', { jobId, paused })
/** Writes a report into `dir`; separators in `name` are replaced so exports stay in place. */
export const exportText = (dir: string, name: string, content: string) =>
  invoke<string>('export_text', { dir, name, content })

/** Retries a denied operation through the elevated helper; resolves when it finishes. */
export function opRetryElevated(request: OpRequest) {
  return invoke<ElevatedReport>('op_retry_elevated', {
    request: {
      opId: request.opId,
      kind: request.kind,
      sources: request.sources,
      targetDir: request.targetDir ?? null,
      newName: request.newName ?? null,
      newNames: request.newNames ?? null,
      permanent: request.permanent ?? false,
      overwrite: request.overwrite ?? false,
    },
  })
}

/** Taskbar progress for long operations; `state` is normal/error/paused/none. */
export const taskbarProgress = (state: string, completed: number, total: number) =>
  invoke<void>('taskbar_progress', { state, completed, total })
/** System toast plus a taskbar flash, for an operation that finished in the background. */
export const notifyOp = (title: string, body: string) => invoke<void>('notify_op', { title, body })

/** Reports seen in the last few seconds, so a render loop cannot flood the log file. */
const reported = new Map<string, number>()

/** Writes a frontend error/crash report to the same log the backend uses. */
export function logFrontend(level: 'error' | 'warn' | 'info' | 'debug', message: string, detail: string) {
  const key = `${level}|${message}|${detail.slice(0, 200)}`
  const now = Date.now()
  const last = reported.get(key)
  if (last !== undefined && now - last < 5000) return
  reported.set(key, now)
  if (reported.size > 200) reported.clear()
  void invoke<void>('log_frontend', { level, message, detail }).catch(() => undefined)
}

/** Absolute path of the log folder, or null before the logger is initialised. */
export const logDirPath = () => invoke<string | null>('log_dir_path')

/** Writes a diagnostics zip into `dir` and returns its path. */
export const diagnosticsExport = (dir: string) => invoke<string>('diagnostics_export', { dir })

export type ExtractMenuLabels = { title: string; here: string; to: string }
export type CompressMenuLabels = { title: string; formats: { id: CompressFormat; label: string }[] }
/** Formats the Rust writer can produce; ids are the contract with the backend. */
export type CompressFormat = 'zip' | '7z' | 'tar.gz' | 'tar.xz' | 'tar.zst'
export const COMPRESS_FORMATS: CompressFormat[] = ['zip', '7z', 'tar.gz', 'tar.xz', 'tar.zst']
export const COMPRESS_EXTENSIONS: Record<CompressFormat, string> = {
  zip: '.zip',
  '7z': '.7z',
  'tar.gz': '.tar.gz',
  'tar.xz': '.tar.xz',
  'tar.zst': '.tar.zst',
}
/**
 * `extract` adds FlowDir's extraction submenu (archives only), `compress` its compression
 * submenu (any selection).
 */
export const shellMenuShow = (
  paths: string[],
  x: number,
  y: number,
  extended: boolean,
  hashLabel?: string,
  extract?: ExtractMenuLabels,
  compress?: CompressMenuLabels,
) =>
  invoke<MenuOutcome>('shell_menu_show', {
    paths,
    x,
    y,
    extended,
    hashLabel: hashLabel ?? null,
    extractTitle: extract?.title ?? null,
    extractHere: extract?.here ?? null,
    extractTo: extract?.to ?? null,
    compress: compress ?? null,
  })
export const shellMenuBackgroundShow = (
  path: string,
  x: number,
  y: number,
  extended: boolean,
  toggleLabel: string,
  dupLabel: string,
  compareLabel: string,
) =>
  invoke<MenuOutcome>('shell_menu_background_show', {
    path,
    x,
    y,
    extended,
    toggleLabel,
    dupLabel,
    compareLabel,
  })
export const appInfo = () => invoke<AppInfo>('app_info')
export const updateCheck = (force: boolean) => invoke<UpdateStatus>('update_check', { force })
export const updateInstall = () => invoke<void>('update_install')
export const configPath = () => invoke<string>('config_path')

/// Relaunches the app; browser arguments such as GPU acceleration only apply to a new process.
export const restartApp = () => invoke<void>('restart_app')
/** Graceful quit: cancels running operations, flushes layout and settings, then exits. */
export const quitApp = () => invoke<void>('quit_app')
/** Ids of the operations still running, used to confirm before a quit. */
export const activeOps = () => invoke<string[]>('active_ops')
export const pickFolder = (title: string, initial: string) =>
  invoke<string | null>('pick_folder', { title, initial })
export const titlebarMenu = (x: number, y: number) => invoke<void>('titlebar_menu', { x, y })

export type SearchFilter = {
  minSize?: number
  maxSize?: number
  afterMs?: number
  beforeMs?: number
  extensions?: string[]
  kind?: '' | 'file' | 'dir'
}
export type ContentOptions = { enabled: boolean; regex: boolean; maxBytes?: number }

export function searchStart(
  root: string,
  query: string,
  gen: number,
  limit: number,
  preferEverything: boolean,
  hidden: boolean,
  filter: SearchFilter,
  content: ContentOptions,
  onBatch: (batch: SearchBatch) => void,
  roots?: string[],
) {
  const channel = new Channel<SearchBatch>()
  channel.onmessage = onBatch
  return invoke<void>('search_start', {
    root,
    roots: roots?.length ? roots : undefined,
    query,
    gen,
    limit,
    preferEverything,
    hidden,
    filter: {
      minSize: filter.minSize ?? 0,
      maxSize: filter.maxSize ?? 0,
      afterMs: filter.afterMs ?? 0,
      beforeMs: filter.beforeMs ?? 0,
      extensions: filter.extensions ?? [],
      kind: filter.kind ?? '',
    },
    content: {
      enabled: content.enabled,
      regex: content.regex,
      maxBytes: content.maxBytes ?? 10 * 1024 * 1024,
    },
    onBatch: channel,
  })
}

export type CompareOptions = {
  includeHidden: boolean
  extensions: string[]
  minSize: number
  maxSize: number
  afterMs: number
  beforeMs: number
  exclude: string[]
  toleranceMs: number
}

export const compareLevel = (
  left: string,
  right: string,
  prefix: string,
  options: CompareOptions,
) => invoke<CompareLevel>('compare_level', { left, right, prefix, options })

export function compareCountsStart(
  jobId: string,
  left: string,
  right: string,
  prefix: string,
  options: CompareOptions,
  onEvent: (event: CompareCount) => void,
) {
  const channel = new Channel<CompareCount>()
  channel.onmessage = onEvent
  return invoke<void>('compare_counts_start', {
    jobId,
    left,
    right,
    prefix,
    options,
    onEvent: channel,
  })
}

export function compareHashStart(
  jobId: string,
  pairs: CompareHashPair[],
  maxBytesPerSec: number,
  onEvent: (event: CompareHashEvent) => void,
) {
  const channel = new Channel<CompareHashEvent>()
  channel.onmessage = onEvent
  return invoke<void>('compare_hash_start', {
    jobId,
    pairs,
    maxBytesPerSec,
    onEvent: channel,
  })
}

export type EverythingStatus = { usable: boolean; version: string | null; reason: string | null }
export const everythingStatus = () => invoke<EverythingStatus>('everything_status')
export const shellProperties = (path: string) => invoke<void>('shell_properties', { path })
/** Invokes the shell's Eject verb on a drive root; removal itself is asynchronous. */
export const driveEject = (root: string) => invoke<void>('drive_eject', { root })
/** Puts recycled files back at their original paths; reports the ones no longer in the bin. */
export const restoreRecycle = (paths: string[]) =>
  invoke<{ restored: string[]; missing: string[] }>('restore_recycle', { paths })

export const searchCancel = () => invoke<void>('search_cancel')
/** Preview payload; `hydrate` reads a cloud placeholder, which downloads it. */
export const previewInfo = (path: string, width: number, bg: string, hydrate = false) =>
  invoke<PreviewInfo>('preview_info', { path, width, bg, hydrate })
export const previewExternal = (path: string) => invoke<void>('preview_external', { path })
