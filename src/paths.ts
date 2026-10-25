export const joinPath = (dir: string, name: string) =>
  dir.endsWith('\\') ? `${dir}${name}` : `${dir}\\${name}`

export function parentPath(p: string): string {
  const norm = p.replace(/\\+$/, '')
  if (/^[a-zA-Z]:$/.test(norm)) return `${norm}\\`
  const idx = norm.lastIndexOf('\\')
  if (idx <= 1) return `${norm.slice(0, 2)}\\`
  const up = norm.slice(0, idx)
  return /^[a-zA-Z]:$/.test(up) ? `${up}\\` : up
}

export function crumbs(p: string): { label: string; path: string }[] {
  const norm = p.replace(/\\+$/, '') || p
  const parts = norm.split('\\')
  const out: { label: string; path: string }[] = []
  let acc = ''
  parts.forEach((part, i) => {
    acc = i === 0 ? `${part}\\` : acc.endsWith('\\') ? `${acc}${part}` : `${acc}\\${part}`
    out.push({ label: part, path: i === 0 ? `${part}\\` : acc })
  })
  return out
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
