import { useEffect, useMemo, useRef, useState } from 'react'
import { dupStart, exportText, hashStart, pickFolder, toolCancel, toolPause } from './api'
import { makeT, type Translate } from './i18n'
import type { DupGroup, DupProgress, HashProgress } from './types'
import type { Lang } from './types'

type Props = {
  lang: Lang
  mode: 'hash' | 'dup'
  /** Files to hash; ignored in duplicate mode. */
  paths: string[]
  /** Folder the duplicate scan starts from. */
  root: string
  includeHidden: boolean
  dupConcurrency: number
  dupMaxMbPerSec: number
  dupExclude: string[]
  onDelete: (paths: string[]) => void
  onToast: (text: string) => void
  onClose: () => void
}

const formatBytes = (bytes: number) => {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${value < 10 && unit > 0 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`
}

const formatDuration = (ms: number) => {
  const seconds = Math.max(0, Math.round(ms / 1000))
  if (seconds < 60) return `${seconds}s`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`
}

const csvCell = (value: string | number) => {
  const text = String(value)
  return /[",\r\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text
}

export function FileToolsPanel({
  lang,
  mode,
  paths,
  root,
  includeHidden,
  dupConcurrency,
  dupMaxMbPerSec,
  dupExclude,
  onDelete,
  onToast,
  onClose,
}: Props) {
  const t = useMemo(() => makeT(lang), [lang])
  const jobId = useRef(`tool-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`)
  const [algorithm, setAlgorithm] = useState('sha256')
  const [compareHash, setCompareHash] = useState('')
  const [minSizeKb, setMinSizeKb] = useState(1)
  const [hidden, setHidden] = useState(includeHidden)
  const [hashProgress, setHashProgress] = useState<HashProgress | null>(null)
  const [dupProgress, setDupProgress] = useState<DupProgress | null>(null)
  const [running, setRunning] = useState(false)
  const [paused, setPaused] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [exporting, setExporting] = useState(false)
  const started = useRef(Date.now())
  /// Speed is sampled locally because the backend only reports cumulative bytes.
  const sample = useRef({ at: Date.now(), bytes: 0, speed: 0 })
  const aborted = useRef(false)

  const hash = mode === 'hash'
  const progress = hash ? hashProgress : dupProgress
  const activeProgress = progress

  useEffect(() => {
    if (!activeProgress) return
    const now = Date.now()
    const delta = now - sample.current.at
    if (delta >= 300) {
      const bytes = activeProgress.bytes - sample.current.bytes
      sample.current = { at: now, bytes: activeProgress.bytes, speed: (bytes * 1000) / delta }
    }
  }, [activeProgress])

  const start = async () => {
    jobId.current = `tool-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
    setError(null)
    setRunning(true)
    setPaused(false)
    aborted.current = false
    started.current = Date.now()
    sample.current = { at: Date.now(), bytes: 0, speed: 0 }
    try {
      if (hash) {
        if (!paths.length) {
          setRunning(false)
          return
        }
        await hashStart(jobId.current, paths, algorithm, (next) => {
          if (next.done) {
            setRunning(false)
            aborted.current = next.aborted
          }
          setHashProgress(next)
        })
      } else {
        await dupStart(
          jobId.current,
          {
            roots: [root],
            includeHidden: hidden,
            minSize: Math.max(1, Math.round(minSizeKb * 1024)),
            concurrency: dupConcurrency,
            maxBytesPerSec: dupMaxMbPerSec * 1024 * 1024,
            exclude: dupExclude,
          },
          (next) => {
            if (next.done) {
              setRunning(false)
              aborted.current = next.aborted
              if (next.groups.length) setSelected(defaultSelection(next.groups))
            }
            setDupProgress(next)
          },
        )
      }
    } catch (err) {
      setRunning(false)
      setError(err && typeof err === 'object' && 'code' in err ? String((err as { code: string }).code) : String(err))
    }
  }

  // The scan needs a user-visible start so the knobs above it are not unreachable.
  useEffect(() => {
    void start()
    // The job is keyed to this panel instance; re-running on prop changes would fight the
    // controls the user is editing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const defaultSelection = (groups: DupGroup[]) => {
    const next = new Set<string>()
    for (const group of groups) {
      // Keep the newest member by default; deleting the copy most recently changed is the
      // riskier choice, and the checkboxes let the user override it.
      const newest = [...group.files].sort((a, b) => b.mtime - a.mtime)[0]
      for (const file of group.files) if (file.path !== newest.path) next.add(file.path)
    }
    return next
  }

  const toggle = (path: string) => {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(path)) {
        next.delete(path)
        return next
      }
      // At least one member must stay behind, otherwise "deduplicate" would delete the
      // file itself along with its copies.
      const group = (dupProgress?.groups ?? []).find((g) => g.files.some((f) => f.path === path))
      const keptAfter = group?.files.some((f) => f.path !== path && !next.has(f.path)) ?? true
      if (!keptAfter) return prev
      next.add(path)
      return next
    })
  }

  const cancel = () => {
    aborted.current = true
    void toolCancel(jobId.current)
    setPaused(false)
    setRunning(false)
  }

  const togglePause = () => {
    const next = !paused
    setPaused(next)
    void toolPause(jobId.current, next)
  }

  const copyHashResults = async () => {
    const results = hashProgress?.results ?? []
    if (!results.length) return
    const text = results
      .map((row) => (row.error ? `${t('tools.failed')}  ${row.path}` : `${row.hash}  ${row.path}`))
      .join('\r\n')
    await navigator.clipboard.writeText(text)
    onToast(t('tools.copyDone'))
  }

  const exportCsv = async () => {
    if (exporting) return
    setExporting(true)
    try {
      const dir = await pickFolder(t('tools.export'), root)
      if (!dir) return
      const stamp = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19)
      let content: string
      let name: string
      if (hash) {
        const rows = hashProgress?.results ?? []
        content = ['algorithm,hash,size,path,error']
          .concat(rows.map((row) => [row.algorithm, row.hash, row.size, row.path, row.error ?? ''].map(csvCell).join(',')))
          .join('\r\n')
        name = `flowdir-hash-${stamp}.csv`
      } else {
        const rows = (dupProgress?.groups ?? []).flatMap((group, index) =>
          group.files.map((file) =>
            [index + 1, group.size, group.hash, file.mtime, file.path].map(csvCell).join(','),
          ),
        )
        content = ['group,size,sha256,mtime,path'].concat(rows).join('\r\n')
        name = `flowdir-duplicates-${stamp}.csv`
      }
      const written = await exportText(dir, name, content)
      onToast(t('tools.exported', { path: written }))
    } catch (err) {
      setError(String(err))
    } finally {
      setExporting(false)
    }
  }

  const deleteSelected = () => {
    if (!selected.size) return
    const message = t('tools.confirmDelete', { count: selected.size, size: formatBytes(selectedBytes) })
    if (!window.confirm(message)) return
    onDelete(Array.from(selected))
    onClose()
  }

  const selectedBytes = useMemo(() => {
    if (hash) return 0
    let total = 0
    for (const group of dupProgress?.groups ?? [])
      for (const file of group.files) if (selected.has(file.path)) total += file.size
    return total
  }, [hash, dupProgress, selected])

  const phaseLabel = () => {
    if (hash) return t('tools.hashTitle')
    switch (dupProgress?.phase) {
      case 'scan':
        return t('tools.scanning')
      case 'quick':
        return t('tools.quick')
      case 'full':
        return t('tools.full')
      default:
        return t('tools.dupTitle')
    }
  }

  const percent = progress && progress.total > 0 ? Math.min(100, (progress.current / progress.total) * 100) : 0
  const speed = sample.current.speed
  const done = !!progress?.done

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4" onMouseDown={onClose}>
      <div
        className="flex max-h-[82vh] w-[46rem] max-w-full flex-col overflow-hidden rounded-xl border border-line bg-surface shadow-2xl"
        onMouseDown={(ev) => ev.stopPropagation()}
        data-file-tools
      >
        <div className="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2">
          <div className="min-w-0 flex-1">
            <div className="text-[0.8125rem] font-semibold text-fg-strong">
              {hash ? t('tools.hashTitle') : t('tools.dupTitle')}
            </div>
            <div className="truncate text-[0.65625rem] text-muted-2">
              {hash ? `${paths.length} ${t('tools.hashTitle')}` : t('tools.root', { path: root })}
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label={t('tools.close')}
            className="grid h-6 w-6 place-items-center rounded text-muted hover:bg-surface-3 hover:text-fg"
          >
            ×
          </button>
        </div>

        <div className="shrink-0 space-y-2 border-b border-line px-3 py-2">
          {hash ? (
            <>
              <div className="flex items-center gap-2 text-[0.6875rem] text-muted">
                <span>{t('tools.algorithm')}</span>
                <select
                  value={algorithm}
                  onChange={(ev) => setAlgorithm(ev.target.value)}
                  className="h-6 rounded border border-line bg-surface-2 px-1 text-fg outline-none"
                >
                  <option value="sha256">SHA-256</option>
                  <option value="sha1">SHA-1</option>
                  <option value="md5">MD5</option>
                </select>
                <span className="ml-auto text-muted-2">{t('tools.elapsed', { time: formatDuration(Date.now() - started.current) })}</span>
              </div>
              <div className="flex items-center gap-2 text-[0.6875rem]">
                <span className="shrink-0 text-muted">{t('tools.compareHash')}</span>
                <input
                  value={compareHash}
                  onChange={(ev) => setCompareHash(ev.target.value)}
                  placeholder={t('tools.comparePh')}
                  className="h-6 min-w-0 flex-1 rounded border border-line bg-surface-2 px-1.5 font-mono text-[0.6875rem] text-fg outline-none focus:border-blue-400"
                />
                <button
                  type="button"
                  onClick={() => {
                    void navigator.clipboard?.readText?.().then((text) => {
                      if (text) setCompareHash(text.trim())
                    })
                  }}
                  className="shrink-0 rounded border border-line px-2 py-0.5 text-[0.6875rem] text-muted hover:bg-surface-3 hover:text-fg"
                >
                  {t('tools.pasteClipboard')}
                </button>
                {compareHash.trim() && (
                  <button
                    type="button"
                    onClick={() => setCompareHash('')}
                    className="shrink-0 rounded px-1.5 py-0.5 text-[0.6875rem] text-muted-2 hover:bg-surface-3 hover:text-fg"
                  >
                    ×
                  </button>
                )}
              </div>
            </>
          ) : (
            <div className="flex items-center gap-3 text-[0.6875rem] text-muted">
              <label className="flex items-center gap-1">
                {t('tools.minSize')}
                <input
                  type="number"
                  min={0}
                  value={minSizeKb}
                  onChange={(ev) => setMinSizeKb(Math.max(0, Number(ev.target.value) || 0))}
                  className="h-6 w-14 rounded border border-line bg-surface-2 px-1 text-right tabular text-fg outline-none"
                />
              </label>
              <label className="flex items-center gap-1">
                <input type="checkbox" checked={hidden} onChange={(ev) => setHidden(ev.target.checked)} />
                {t('tools.includeHidden')}
              </label>
              <button
                type="button"
                onClick={() => void start()}
                disabled={running}
                className="rounded border border-line px-2 py-0.5 text-fg hover:bg-surface-3 disabled:opacity-40"
              >
                {t('tools.start')}
              </button>
            </div>
          )}

          <div className="flex items-center gap-2 text-[0.6875rem] text-muted">
            <span className="min-w-0 flex-1 truncate">
              {phaseLabel()}
              {running && progress ? ` · ${progress.current}/${progress.total || '…'}` : ''}
              {paused ? ' · ' : ''}
              {paused ? t('tools.pause') : ''}
            </span>
            {speed > 0 && <span className="tabular text-muted-2">{formatBytes(speed)}/s</span>}
            {running && (
              <>
                <button
                  type="button"
                  onClick={togglePause}
                  className="rounded border border-line px-2 py-0.5 text-fg hover:bg-surface-3"
                >
                  {paused ? t('tools.resume') : t('tools.pause')}
                </button>
                <button
                  type="button"
                  onClick={cancel}
                  className="rounded border border-line px-2 py-0.5 text-fg hover:bg-surface-3"
                >
                  {t('tools.cancel')}
                </button>
              </>
            )}
          </div>
          <div className="h-1 overflow-hidden rounded bg-surface-3">
            <div
              className={`h-full ${paused ? 'bg-amber-400' : 'bg-blue-500'}`}
              style={{ width: `${done ? 100 : percent}%` }}
            />
          </div>
          {error && <div className="text-[0.65625rem] text-red-500">{error}</div>}
        </div>

        <div className="min-h-0 flex-1 overflow-auto px-3 py-2">
          {hash ? (
            <HashTable
              progress={hashProgress}
              compareHash={compareHash}
              onCopyHash={(h) => {
                void navigator.clipboard?.writeText(h).then(() => onToast(t('tools.copyDone')))
              }}
              t={t}
            />
          ) : (
            <DupTable
              progress={dupProgress}
              selected={selected}
              onToggle={toggle}
              t={t}
            />
          )}
        </div>

        <div className="flex shrink-0 items-center gap-2 border-t border-line px-3 py-2 text-[0.6875rem]">
          {!hash && !!dupProgress?.groups.length && (
            <>
              <span className="min-w-0 flex-1 truncate text-muted">
                {t('tools.dupGroups', {
                  groups: dupProgress.groups.length,
                  size: formatBytes(dupProgress.groups.reduce((sum, g) => sum + g.size * (g.files.length - 1), 0)),
                })}
              </span>
              <button
                type="button"
                onClick={deleteSelected}
                disabled={!selected.size}
                className="rounded border border-line px-2 py-1 text-red-500 hover:bg-surface-3 disabled:opacity-40"
              >
                {t('tools.deleteSelected')} ({selected.size}) · {formatBytes(selectedBytes)}
              </button>
            </>
          )}
          {hash && !!hashProgress?.results.length && (
            <button
              type="button"
              onClick={() => void copyHashResults()}
              className="rounded border border-line px-2 py-1 text-fg hover:bg-surface-3"
            >
              {t('tools.copy')}
            </button>
          )}
          <button
            type="button"
            onClick={() => void exportCsv()}
            disabled={exporting || !done}
            className="rounded border border-line px-2 py-1 text-fg hover:bg-surface-3 disabled:opacity-40"
          >
            {t('tools.export')}
          </button>
        </div>
      </div>
    </div>
  )
}

function HashTable({
  progress,
  compareHash,
  onCopyHash,
  t,
}: {
  progress: HashProgress | null
  compareHash: string
  onCopyHash: (hash: string) => void
  t: Translate
}) {
  const rows = progress?.results ?? []
  const target = compareHash.trim().toLowerCase()
  if (!rows.length) return <div className="py-6 text-center text-[0.6875rem] text-muted-2">{progress?.done ? t('tools.noResults') : '…'}</div>
  return (
    <table className="w-full text-left text-[0.65625rem]">
      <thead className="sticky top-0 bg-surface text-muted-2">
        <tr>
          <th className="py-1 pr-2 font-normal">SHA</th>
          <th className="py-1 pr-2 font-normal">Size</th>
          <th className="py-1 font-normal">Path</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => {
          const isMatch = Boolean(target && row.hash && row.hash.toLowerCase() === target)
          const isMismatch = Boolean(target && row.hash && !row.error && row.hash.toLowerCase() !== target)
          return (
            <tr
              key={row.path}
              className={`group border-t border-line/60 transition-colors ${
                isMatch ? 'bg-emerald-500/10' : isMismatch ? 'opacity-80' : ''
              }`}
            >
              <td className="max-w-[18rem] truncate py-1 pr-2 font-mono text-fg" title={row.hash}>
                <div className="flex items-center gap-1.5">
                  <span className={isMatch ? 'font-semibold text-emerald-600 dark:text-emerald-400' : ''}>
                    {row.error ? <span className="text-red-500">{t('tools.hashError')}</span> : row.hash}
                  </span>
                  {isMatch && (
                    <span className="shrink-0 rounded bg-emerald-500/20 px-1 py-0.2 text-[0.625rem] font-semibold text-emerald-600 dark:text-emerald-400">
                      {t('tools.hashMatched')}
                    </span>
                  )}
                  {isMismatch && (
                    <span className="shrink-0 rounded bg-red-500/10 px-1 py-0.2 text-[0.625rem] text-red-400">
                      {t('tools.hashMismatch')}
                    </span>
                  )}
                  {row.hash && (
                    <button
                      type="button"
                      onClick={() => onCopyHash(row.hash)}
                      title={t('tools.copy')}
                      className="shrink-0 rounded px-1 opacity-0 hover:text-blue-500 group-hover:opacity-100"
                    >
                      📋
                    </button>
                  )}
                </div>
              </td>
              <td className="whitespace-nowrap py-1 pr-2 tabular text-muted">{formatBytes(row.size)}</td>
              <td className="truncate py-1 text-fg" title={row.path}>
                {row.path}
              </td>
            </tr>
          )
        })}
      </tbody>
    </table>
  )
}

function DupTable({
  progress,
  selected,
  onToggle,
  t,
}: {
  progress: DupProgress | null
  selected: Set<string>
  onToggle: (path: string) => void
  t: Translate
}) {
  const groups = progress?.groups ?? []
  if (!groups.length)
    return (
      <div className="py-6 text-center text-[0.6875rem] text-muted-2">
        {progress?.done ? t('tools.dupNone') : '…'}
        {progress?.done && (
          <div className="mt-1">
            {t('tools.dupScanned', { count: progress.scanned })} ·{' '}
            {t('tools.dupSkipped', {
              hidden: progress.skippedHidden,
              cloud: progress.skippedCloud,
              hard: progress.skippedHardlink,
            })}
          </div>
        )}
      </div>
    )
  return (
    <div className="space-y-3">
      {groups.map((group, index) => {
        const newest = [...group.files].sort((a, b) => b.mtime - a.mtime)[0]
        return (
          <div key={`${group.hash}-${index}`} className="rounded border border-line">
            <div className="flex items-center gap-2 border-b border-line bg-surface-2 px-2 py-1 text-[0.65625rem] text-muted">
              <span className="font-semibold text-fg">
                {formatBytes(group.size)} × {group.files.length}
              </span>
              <span className="ml-auto truncate font-mono text-muted-2" title={group.hash}>
                {group.hash.slice(0, 16)}…
              </span>
            </div>
            {group.files.map((file) => {
              const isNewest = file.path === newest.path
              return (
                <label
                  key={file.path}
                  className="flex cursor-default items-center gap-2 px-2 py-1 text-[0.65625rem] hover:bg-surface-3"
                >
                  <input
                    type="checkbox"
                    checked={selected.has(file.path)}
                    onChange={() => onToggle(file.path)}
                    data-dup-file={file.path}
                  />
                  <span className="min-w-0 flex-1 truncate text-fg" title={file.path}>
                    {file.path}
                  </span>
                  <span className="shrink-0 text-muted-2">{new Date(file.mtime).toLocaleString()}</span>
                  {isNewest && (
                    <span className="shrink-0 rounded bg-surface-3 px-1 text-[0.59375rem] text-muted">
                      {t('tools.keepNewest')}
                    </span>
                  )}
                </label>
              )
            })}
          </div>
        )
      })}
    </div>
  )
}