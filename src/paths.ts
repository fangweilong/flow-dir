/**
 * Identity key for a path. Windows paths are case-folded; WebDAV addresses keep their
 * case because the server may be case-sensitive (`Photos` and `photos` can coexist).
 * Trailing slashes are dropped so a root address matches its non-slash spelling.
 */
export const pathKey = (path: string) => {
  if (
    path.length >= WEBDAV_SCHEME.length &&
    path.slice(0, WEBDAV_SCHEME.length).toLowerCase() === WEBDAV_SCHEME
  ) {
    return `${WEBDAV_SCHEME}${path.slice(WEBDAV_SCHEME.length).replace(/\/+$/, '')}`
  }
  // Keep `C:\` intact but drop separators elsewhere, so `C:\foo\` and `C:\foo` compare
  // equal in same-folder checks (the address bar can preserve a trailing slash).
  // Windows accepts `/` as a separator, and `samePath` already folds the two spellings;
  // folding here too keeps every path-keyed cache from treating one folder as two.
  const trimmed = (path.length > 3 ? path.replace(/[\\/]+$/, '') : path).replace(/\//g, '\\')
  return trimmed.toLowerCase()
}

/** Virtual address scheme for WebDAV servers, e.g. `webdav://nas/photos`. */
export const WEBDAV_SCHEME = 'webdav://'

let revealSeq = 0

/**
 * One-shot marker for "scroll this row into view".
 *
 * A `PaneView` remembers the last id it handled across tab switches, so the ids must be
 * unique across every producer (launch request, drag-drop, up-one-level pick); separate
 * per-hook counters all started at 1 and made a later reveal look already handled.
 */
export const nextRevealId = () => {
  revealSeq += 1
  return revealSeq
}

/**
 * Connection names for the breadcrumb. The backend stores the stable id in the tab path,
 * so a renamed connection must not require rewriting workspaces.
 */
let webdavDirectory: { id: string; name: string }[] = []

export function setWebdavConnections(list: { id: string; name: string }[]) {
  webdavDirectory = list.map(({ id, name }) => ({ id, name }))
}

/** Display name for a connection id; the id itself when the connection is gone. */
export function webdavConnectionName(id: string): string {
  const found = webdavDirectory.find((entry) => entry.id.toLowerCase() === id.toLowerCase())
  return found?.name || id
}

/** Accepts either an id or a display name in the address bar. */
function webdavId(raw: string): string {
  const found = webdavDirectory.find(
    (entry) =>
      entry.id.toLowerCase() === raw.toLowerCase() || entry.name.toLowerCase() === raw.toLowerCase(),
  )
  return found?.id ?? raw
}

export const isWebdavPath = (path: string) =>
  path.length >= WEBDAV_SCHEME.length &&
  path.slice(0, WEBDAV_SCHEME.length).toLowerCase() === WEBDAV_SCHEME

/** `webdav://id/a/b` → connection `id` and `['a','b']`; `.`/`..` are resolved. */
export function webdavParts(path: string): { id: string; segments: string[] } {
  // Only `/` separates remote path segments. A backslash is a legal character in a
  // remote name, so treating it as a separator would address a different object.
  const rest = path.slice(WEBDAV_SCHEME.length)
  const [rawId, ...tail] = rest.split('/')
  const segments: string[] = []
  for (const part of tail.join('/').split('/')) {
    if (!part || part === '.') continue
    if (part === '..') {
      segments.pop()
      continue
    }
    segments.push(part)
  }
  return { id: webdavId(rawId ?? ''), segments }
}

/** Canonical virtual address: root keeps a trailing slash, deeper paths do not. */
function webdavAddress(path: string): string {
  const { id, segments } = webdavParts(path)
  if (!id) return ''
  return segments.length ? `${WEBDAV_SCHEME}${id}/${segments.join('/')}` : `${WEBDAV_SCHEME}${id}/`
}

/** Paths the panes can list, including shell namespaces and WebDAV servers. */
export const isNavigablePath = (path: string) =>
  isFsPath(path) || isShellPath(path) || isWebdavPath(path)

/**
 * Resolves a typed address against the current folder. Absolute drives, UNC paths and
 * `webdav://` addresses pass through, while `.`, `..` and nested names are joined and
 * normalized; quotes pasted from Explorer are stripped. Returns '' only for an empty input.
 */
export function resolveAddress(base: string, raw: string): string {
  let text = raw.trim()
  const quoted = text.match(/^"(.*)"$/)
  if (quoted) text = quoted[1].trim()
  if (!text) return ''
  if (isWebdavPath(text)) return webdavAddress(text)
  // Absolute Windows targets are recognised with either separator, as before.
  const windowsText = text.replace(/\//g, '\\')
  if (/^[a-zA-Z]:$/.test(windowsText)) return `${windowsText}\\`
  if (
    /^[a-zA-Z]:/.test(windowsText) ||
    windowsText.startsWith('\\\\') ||
    isShellPath(windowsText)
  ) {
    return windowsText
  }
  // A remote base resolves relative names with forward slashes, not with the Windows
  // separator the local branch below uses.
  if (isWebdavPath(base)) {
    return webdavAddress(`${base.replace(/\/+$/, '')}/${text.replace(/\\/g, '/')}`)
  }
  text = windowsText
  // A shell location has no parent to resolve relative names against.
  if (!base || isShellPath(base)) return text

  const combined = `${base.replace(/\\+$/, '')}\\${text}`
  const unc = combined.startsWith('\\\\')
  const drive = /^[a-zA-Z]:/.test(combined) ? combined.slice(0, 2) : ''
  const body = unc || drive ? combined.slice(2) : combined
  const parts: string[] = []
  // `..` may never climb above a drive root or a server/share pair.
  const floor = unc ? 2 : 0
  for (const part of body.split('\\')) {
    if (!part || part === '.') continue
    if (part === '..') {
      if (parts.length > floor) parts.pop()
      continue
    }
    parts.push(part)
  }
  if (unc) return `\\\\${parts.join('\\')}`
  if (drive) return `${drive}\\${parts.join('\\')}`
  return parts.join('\\')
}

export const joinPath = (dir: string, name: string) => {
  if (isWebdavPath(dir)) {
    return `${dir.replace(/\/+$/, '')}/${name.replace(/^\/+/, '')}`
  }
  return dir.endsWith('\\') ? `${dir}${name}` : `${dir}\\${name}`
}

/** Shell-namespace location (recycle bin, control panel) rather than a filesystem path. */
export const isShellPath = (path: string) => path.startsWith('shell:') || path.startsWith('::')

const RESERVED_NAME = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])$/i
const ILLEGAL_NAME = /[\u0000-\u001f<>:"/\\|?*]/

/**
 * True for a name Windows cannot create as a single path segment. Shared by the dialogs
 * that let the user type a file/folder/archive name so a `..\..\x` spelling can never
 * escape the folder they picked.
 */
export function invalidFileName(name: string): boolean {
  // Windows reserves a device name when the segment before the *first* dot matches, so
  // `con.tar.gz` is as blocked as `con`; the archive writer resolves the stem the same way.
  const stem = name.split('.')[0]
  return (
    !name ||
    // The backend measures the same limit in UTF-16 code units (`encode_utf16`), which is
    // what JS `length` counts, so a 256-unit name would fail after the preview said "ok".
    name.length > 255 ||
    ILLEGAL_NAME.test(name) ||
    /[ .]$/.test(name) ||
    name === '.' ||
    name === '..' ||
    RESERVED_NAME.test(stem)
  )
}

/** Full path of a row: shell items carry their own parsing name, filesystem rows join. */
export const entryPath = (dir: string, entry: { name: string; href?: string }) =>
  entry.href || joinPath(dir, entry.name)

/** Short label for chrome that has no room for a full path: the last segment, or a connection name. */
export function pathLabel(path: string): string {
  if (isWebdavPath(path)) {
    const { id, segments } = webdavParts(path)
    return segments.length ? segments[segments.length - 1] : webdavConnectionName(id)
  }
  return path.split('\\').filter(Boolean).pop() ?? path
}

/** Trailing segment of a path, for building a destination from a source path. */
export const baseName = (path: string) => {
  const cut = Math.max(path.lastIndexOf('\\'), path.lastIndexOf('/'))
  return cut < 0 ? path : path.slice(cut + 1)
}

export function parentPath(p: string): string {
  if (isWebdavPath(p)) {
    const { id, segments } = webdavParts(p)
    if (!segments.length) return ''
    segments.pop()
    return segments.length ? `${WEBDAV_SCHEME}${id}/${segments.join('/')}` : `${WEBDAV_SCHEME}${id}/`
  }
  // A command-line target keeps the user's spelling (`flowdir D:/a/b`), and Windows treats
  // `/` as a separator; splitting on `\` alone jumped straight to the drive root.
  const norm = p.replace(/\//g, '\\').replace(/\\+$/, '')
  // The drive overview is the top; a drive or share root climbs back to it.
  if (!norm || isShellPath(norm)) return ''
  if (/^[a-zA-Z]:$/.test(norm)) return ''
  if (norm.startsWith('\\\\')) {
    const parts = norm.slice(2).split('\\')
    return parts.length <= 2 ? '' : `\\\\${parts.slice(0, -1).join('\\')}`
  }
  const idx = norm.lastIndexOf('\\')
  if (idx <= 1) return `${norm.slice(0, 2)}\\`
  const up = norm.slice(0, idx)
  return /^[a-zA-Z]:$/.test(up) ? `${up}\\` : up
}

/** Drive root (`C:\`) or UNC share root (`\\server\share`) that contains `p`. */
export function volumeRoot(p: string): string {
  if (isWebdavPath(p)) return `${WEBDAV_SCHEME}${webdavParts(p).id}/`
  const norm = p.replace(/\//g, '\\')
  if (isShellPath(norm)) return ''
  if (norm.startsWith('\\\\')) {
    const parts = norm.slice(2).split('\\').filter(Boolean)
    return parts.length >= 2 ? `\\\\${parts[0]}\\${parts[1]}` : norm
  }
  const drive = norm.match(/^[a-zA-Z]:/)
  return drive ? `${drive[0]}\\` : ''
}

/** A folder followed by its ancestors, deepest first: the nodes a tree must expand. */
export function treeChain(path: string): string[] {
  const out: string[] = []
  let cur = path
  while (cur) {
    out.push(cur)
    cur = parentPath(cur)
  }
  return out
}

export function crumbs(p: string): { label: string; path: string }[] {
  if (isWebdavPath(p)) {
    const { id, segments } = webdavParts(p)
    const out = [{ label: webdavConnectionName(id), path: `${WEBDAV_SCHEME}${id}/` }]
    let acc = `${WEBDAV_SCHEME}${id}`
    for (const segment of segments) {
      acc = `${acc}/${segment}`
      out.push({ label: segment, path: acc })
    }
    return out
  }
  // Shell locations are one opaque node; their parsing name is not a filesystem hierarchy.
  if (isShellPath(p)) return [{ label: p, path: p }]
  const norm = p.replace(/\//g, '\\').replace(/\\+$/, '') || p
  if (norm.startsWith('\\\\')) {
    const parts = norm.slice(2).split('\\')
    const root = `\\\\${parts[0]}\\${parts[1] ?? ''}`.replace(/\\$/, '')
    const out = [{ label: root, path: root }]
    let acc = root
    for (const part of parts.slice(2)) {
      acc = `${acc}\\${part}`
      out.push({ label: part, path: acc })
    }
    return out
  }
  const parts = norm.split('\\')
  const out: { label: string; path: string }[] = []
  let acc = ''
  parts.forEach((part, i) => {
    acc = i === 0 ? `${part}\\` : acc.endsWith('\\') ? `${acc}${part}` : `${acc}\\${part}`
    out.push({ label: part, path: i === 0 ? `${part}\\` : acc })
  })
  return out
}

/** Paths the built-in list can open; network locations may point at URLs the shell handles. */
export const isFsPath = (path: string) => {
  const norm = path.replace(/\//g, '\\')
  return /^[a-zA-Z]:/.test(norm) || norm.startsWith('\\\\')
}

/** True when `path` is `root` or sits inside it; `C:\x` does not match `C:\xy`. */
export function underPath(path: string, root: string): boolean {
  if (isWebdavPath(path) || isWebdavPath(root)) {
    // WebDAV servers may be case-sensitive, so this branch keeps the original spelling
    // (matching `pathKey`) and only drops trailing separators.
    const p = path.replace(/\/+$/, '')
    const r = root.replace(/\/+$/, '')
    return p === r || p.startsWith(`${r}/`)
  }
  const p = path.replace(/\//g, '\\').toLowerCase()
  const r = root.replace(/\//g, '\\').toLowerCase()
  return p === r || p.startsWith(r.endsWith('\\') ? r : `${r}\\`)
}

/**
 * Windows drop semantics: a drag inside one volume moves, across volumes copies. UNC
 * paths count as their `\\server\share` root, which is the closest thing to a volume.
 */
export function sameVolume(a: string, b: string): boolean {
  if (isWebdavPath(a) || isWebdavPath(b)) {
    // One connection behaves like one volume: drag inside it moves, across servers copies.
    const connection = (path: string) =>
      isWebdavPath(path) ? webdavParts(path).id.toLowerCase() : ''
    const left = connection(a)
    return left !== '' && left === connection(b)
  }
  const root = (path: string): string => {
    const drive = /^[a-zA-Z]:/.exec(path)
    if (drive) return drive[0].toLowerCase()
    if (path.startsWith('\\\\')) {
      const parts = path.slice(2).split('\\')
      return `\\\\${(parts[0] ?? '').toLowerCase()}\\${(parts[1] ?? '').toLowerCase()}`
    }
    return ''
  }
  const left = root(a)
  return left !== '' && left === root(b)
}

export const isDir = (attrs: number) => (attrs & 0x10) !== 0

/** Archives the backend can extract in-process; 7z/rar/cab are not listed yet. */
const ARCHIVE_EXTS = [
  'zip',
  '7z',
  'cab',
  'rar',
  'iso',
  'tar',
  'tgz',
  'tbz',
  'tbz2',
  'txz',
  'tzst',
  'gz',
  'bz2',
  'xz',
  'zst',
]
export const isArchiveName = (name: string) => {
  const lower = name.toLowerCase()
  return ARCHIVE_EXTS.some((ext) => lower.endsWith(`.${ext}`))
}

/**
 * Directories the backend reported as archive listings. A directory whose name merely
 * looks like an archive (`C:\data\backup.zip\`) is a normal folder and must not be
 * mistaken for one.
 */
const virtualRoots = new Set<string>()

/** Records the scan result for `path`, so ancestor checks reflect what the backend did. */
export function markVirtualRoot(path: string, virtual: boolean) {
  const key = pathKey(path).replace(/\\+$/, '')
  if (virtual) virtualRoots.add(key)
  else virtualRoots.delete(key)
}

/** True when `path` is an archive file or lies inside a path the backend listed virtually. */
export function isInsideArchive(path: string): boolean {
  if (!path) return false
  let cur = pathKey(path).replace(/\\+$/, '')
  while (cur) {
    if (virtualRoots.has(cur)) return true
    const cut = cur.lastIndexOf('\\')
    if (cut <= 0) return false
    cur = cur.slice(0, cut)
  }
  return false
}

/**
 * True when `path` is an entry inside an archive listed this session, excluding the
 * archive file itself. Copy/drag decisions need this variant: the archive was opened
 * once, so `isInsideArchive` also matches the archive path and would route the file
 * itself to the extraction path, which skips it.
 */
export function isArchiveMember(path: string): boolean {
  const parent = parentPath(path)
  return parent ? isInsideArchive(parent) : false
}

/**
 * True when an ancestor folder name carries an archive extension.
 *
 * Only for clipboard paths restored after a restart, where the in-session virtual roots
 * are gone. A real folder named `data.zip` also matches, so the caller must confirm the
 * path itself does not exist before treating it as an archive member.
 */
export function hasArchiveAncestor(path: string): boolean {
  let cur = pathKey(path).replace(/\\+$/, '')
  for (;;) {
    const cut = cur.lastIndexOf('\\')
    if (cut <= 0) return false
    cur = cur.slice(0, cut)
    if (isArchiveName(cur.slice(cur.lastIndexOf('\\') + 1))) return true
  }
}

export const formatSize = (n: number, dir: boolean) => {
  if (dir) return ''
  if (!Number.isFinite(n) || n < 0 || n > 1e15) return '—'
  if (n < 1024) return `${n} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let v = n / 1024
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i += 1
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`
}

export const formatTime = (ms: number) => {
  if (!ms) return ''
  const d = new Date(ms)
  const pad = (v: number) => String(v).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}
