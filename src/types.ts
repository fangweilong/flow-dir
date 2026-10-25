export type ApiError = { code: string; detail?: string }
export type RawEntry = [string, number, number, number, number]
export type DirBatch = {
  gen: number
  scanned: number
  done: boolean
  items: RawEntry[]
  error: ApiError | null
}
export type Drive = { root: string; kind: string }
export type Place = { key: string; path: string }
export type Lang = 'zh-CN' | 'en'
export type Theme = 'system' | 'light' | 'dark'
export type Dependency = { name: string; version: string; license: string; kind: string }
export type Clip = { paths: string[]; cut: boolean }
export type EverythingStatus = { usable: boolean; version: string | null; reason: string | null }
export type MenuOutcome = { invoked: boolean; id: number }
export type OpKind = 'copy' | 'move' | 'delete' | 'rename' | 'newdir'
export type OpProgress = {
  opId: string
  phase: string
  current: number
  total: number
  name: string | null
  work: number
  workTotal: number
  done: boolean
  aborted: boolean
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

export type Entry = { name: string; size: number; mtime: number; attrs: number; id: number }

export type SortKey = 'name' | 'size' | 'mtime' | 'type'
export type SortDir = 'asc' | 'desc'
export type SortState = { key: SortKey; dir: SortDir }

export type ListData = {
  entries: Entry[]
  loading: boolean
  scanned: number
  error: ApiError | null
  selected: Set<string>
  anchor: number
  filter: string
}

export const emptyList = (): ListData => ({
  entries: [],
  loading: true,
  scanned: 0,
  error: null,
  selected: new Set(),
  anchor: -1,
  filter: '',
})

export const EMPTY_LIST: ListData = emptyList()

export type ArchiveEntry = { name: string; size: number }
export type MetaRow = { label: string; value: string }
export type PreviewInfo = {
  kind: 'image' | 'pdf' | 'video' | 'text' | 'archive' | 'binary' | 'folder' | 'unsupported'
  url?: string | null
  text?: string | null
  entries?: ArchiveEntry[] | null
  meta?: MetaRow[] | null
  reason?: string | null
  size: number
  mtime: number
}
export type SearchItem = { path: string; name: string; isDir: boolean; size: number; mtime: number }
export type SearchBatch = {
  gen: number
  provider: string
  items: SearchItem[]
  done: boolean
  error?: ApiError | null
}
