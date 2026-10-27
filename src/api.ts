import { Channel, invoke } from '@tauri-apps/api/core'
import type { AppInfo, Clip, DirBatch, Drive, MenuOutcome, OpKind, OpProgress, Place, UpdateStatus } from './types'
import type { GitStatus, PreviewInfo, SearchBatch } from './types'
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
/** Repository status for the folder; empty when it is not a work tree. */
export const gitStatus = (path: string) => invoke<GitStatus>('git_status', { path })
export const quickAccess = () => invoke<Place[]>('quick_access')
export const openPath = (path: string) => invoke<void>('open_path', { path })
export const revealPath = (path: string) => invoke<void>('reveal_path', { path })
export const explorerOpen = (path: string) => invoke<void>('explorer_open', { path })

export const layoutSave = (workspace: unknown) => invoke<void>('layout_save', { workspace })
export const layoutLoad = () => invoke<unknown>('layout_load')
export const layoutPreserve = () => invoke<string>('layout_preserve')

export const settingsLoad = () => invoke<Settings>('settings_load')
export const settingsSave = (settings: Settings) => invoke<void>('settings_save', { settings })
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
    onProgress: channel,
  })
}

export const opCancel = (opId: string) => invoke<void>('op_cancel', { opId })

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

export const shellMenuShow = (paths: string[], x: number, y: number, extended: boolean) =>
  invoke<MenuOutcome>('shell_menu_show', { paths, x, y, extended })
export const shellMenuBackgroundShow = (
  path: string,
  x: number,
  y: number,
  extended: boolean,
  toggleLabel: string,
) => invoke<MenuOutcome>('shell_menu_background_show', { path, x, y, extended, toggleLabel })
export const appInfo = () => invoke<AppInfo>('app_info')
export const updateCheck = (force: boolean) => invoke<UpdateStatus>('update_check', { force })
export const updateInstall = () => invoke<void>('update_install')
export const configPath = () => invoke<string>('config_path')

/// Relaunches the app; browser arguments such as GPU acceleration only apply to a new process.
export const restartApp = () => invoke<void>('restart_app')
export const pickFolder = (title: string, initial: string) =>
  invoke<string | null>('pick_folder', { title, initial })
export const titlebarMenu = (x: number, y: number) => invoke<void>('titlebar_menu', { x, y })

export function searchStart(
  root: string,
  query: string,
  gen: number,
  limit: number,
  preferEverything: boolean,
  hidden: boolean,
  onBatch: (batch: SearchBatch) => void,
) {
  const channel = new Channel<SearchBatch>()
  channel.onmessage = onBatch
  return invoke<void>('search_start', {
    root,
    query,
    gen,
    limit,
    preferEverything,
    hidden,
    onBatch: channel,
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
export const previewInfo = (path: string, width: number, bg: string) =>
  invoke<PreviewInfo>('preview_info', { path, width, bg })
export const previewExternal = (path: string) => invoke<void>('preview_external', { path })
