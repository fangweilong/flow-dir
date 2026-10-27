/** Case-insensitive identity for a Windows path, used as a cache/render key. */
export const pathKey = (path: string) => path.toLowerCase()

/**
 * Resolves a typed address against the current folder. Absolute drives and UNC paths pass
 * through, while `.`, `..` and nested names are joined and normalized; quotes pasted from
 * Explorer are stripped. Returns '' only for an empty input.
 */
export function resolveAddress(base: string, raw: string): string {
  let text = raw.trim()
  const quoted = text.match(/^"(.*)"$/)
  if (quoted) text = quoted[1].trim()
  if (!text) return ''
  text = text.replace(/\//g, '\\')
  if (/^[a-zA-Z]:$/.test(text)) return `${text}\\`
  if (/^[a-zA-Z]:/.test(text) || text.startsWith('\\\\')) return text
  if (!base) return text

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

export const joinPath = (dir: string, name: string) =>
  dir.endsWith('\\') ? `${dir}${name}` : `${dir}\\${name}`

/** Trailing segment of a path, for building a destination from a source path. */
export const baseName = (path: string) => {
  const cut = Math.max(path.lastIndexOf('\\'), path.lastIndexOf('/'))
  return cut < 0 ? path : path.slice(cut + 1)
}

export function parentPath(p: string): string {
  const norm = p.replace(/\\+$/, '')
  // The drive overview is the top; a drive or share root climbs back to it.
  if (!norm) return ''
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
  const norm = p.replace(/\//g, '\\')
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
  const norm = p.replace(/\\+$/, '') || p
  if (p.startsWith('\\\\')) {
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
export const isFsPath = (path: string) => /^[a-zA-Z]:/.test(path) || path.startsWith('\\\\')

/** True when `path` is `root` or sits inside it; `C:\x` does not match `C:\xy`. */
export function underPath(path: string, root: string): boolean {
  const p = path.toLowerCase()
  const r = root.toLowerCase()
  return p === r || p.startsWith(r.endsWith('\\') ? r : `${r}\\`)
}

/**
 * Windows drop semantics: a drag inside one volume moves, across volumes copies. UNC
 * paths count as their `\\server\share` root, which is the closest thing to a volume.
 */
export function sameVolume(a: string, b: string): boolean {
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
