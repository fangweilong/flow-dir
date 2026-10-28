import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { compareCountsStart, compareHashStart, compareLevel, exportText, pickFolder, toolCancel } from './api'
import type { CompareOptions } from './api'
import { makeT } from './i18n'
import { formatSize, joinPath } from './paths'
import { FileIcon } from './icons'
import { BASE_FONT_SIZE, clampFontSize } from './fonts'
import type {
  CompareCount,
  CompareEntry,
  CompareHashEvent,
  CompareHashResult,
  CompareStatus,
  Lang,
} from './types'

/** One confirmed step of the preview; the app executes these through the normal op pipeline. */
export type SyncAction =
  | { kind: 'copy'; source: string; targetDir: string }
  | { kind: 'delete'; paths: string[] }

type Session = { left: string; right: string; options: CompareOptions }

type SyncPlan = {
  actions: SyncAction[]
  conflicts: number
  mismatches: number
}

const STATUS_ORDER: CompareStatus[] = ['left', 'right', 'different', 'same']

/** Inputs are megabytes/days; the backend wants bytes/milliseconds. */
const parseMb = (value: string) => {
  const parsed = Number(value)
  return Number.isFinite(parsed) && parsed > 0 ? Math.round(parsed * 1024 * 1024) : 0
}

const splitList = (value: string) =>
  value
    .split(/[\s,;]+/)
    .map((item) => item.trim())
    .filter(Boolean)

const parentRel = (rel: string) => {
  const cut = rel.lastIndexOf('\\')
  return cut < 0 ? '' : rel.slice(0, cut)
}

const csvCell = (value: string | number) => `"${String(value).replace(/"/g, '""')}"`

type Props = {
  lang: Lang
  initialLeft: string
  initialRight: string
  /** Seeds the hidden-items switch; the panel's own switch is what the compare uses. */
  includeHidden: boolean
  fontSize: number
  /** Byte rate for hash verification, from the duplicate scanner setting. */
  hashRateBytes: number
  onClose: () => void
  onToast: (text: string) => void
  /** Runs the confirmed steps in order and resolves when the queue has drained. */
  onSync: (actions: SyncAction[]) => Promise<void>
}

export function ComparePanel({
  lang,
  initialLeft,
  initialRight,
  includeHidden,
  fontSize,
  hashRateBytes,
  onClose,
  onToast,
  onSync,
}: Props) {
  const t = makeT(lang)
  const iconScale = clampFontSize(fontSize) / BASE_FONT_SIZE
  const [leftRoot, setLeftRoot] = useState(initialLeft)
  const [rightRoot, setRightRoot] = useState(initialRight)
  const [session, setSession] = useState<Session | null>(null)
  const [levelRel, setLevelRel] = useState('')
  const [cache, setCache] = useState<Map<string, CompareEntry[]>>(new Map())
  const [counts, setCounts] = useState<Record<string, CompareCount>>({})
  const [hashResults, setHashResults] = useState<Record<string, CompareHashResult>>({})
  const [hashProgress, setHashProgress] = useState<CompareHashEvent | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [includeHiddenOn, setIncludeHiddenOn] = useState(includeHidden)
  const [hashOn, setHashOn] = useState(false)
  const [toleranceSec, setToleranceSec] = useState('2')
  const [exts, setExts] = useState('')
  const [minMb, setMinMb] = useState('')
  const [maxMb, setMaxMb] = useState('')
  const [days, setDays] = useState('')
  const [exclude, setExclude] = useState('')
  const [showOptions, setShowOptions] = useState(false)
  const [statuses, setStatuses] = useState<Set<CompareStatus>>(new Set(STATUS_ORDER))
  const [plan, setPlan] = useState<SyncPlan | null>(null)
  const [previewOpen, setPreviewOpen] = useState(false)
  const [strategy, setStrategy] = useState<'l2r' | 'r2l' | 'both'>('l2r')
  const [deleteExtras, setDeleteExtras] = useState(false)
  const [executing, setExecuting] = useState(false)
  const [exporting, setExporting] = useState(false)
  const countsJob = useRef<string | null>(null)
  const hashJob = useRef<string | null>(null)
  const seq = useRef(0)
  const levelRelRef = useRef('')
  levelRelRef.current = levelRel

  const options: CompareOptions = useMemo(() => {
    const daysValue = Number(days)
    return {
      includeHidden: includeHiddenOn,
      extensions: splitList(exts).map((ext) => ext.replace(/^\./, '').toLowerCase()),
      minSize: parseMb(minMb),
      maxSize: parseMb(maxMb),
      afterMs: Number.isFinite(daysValue) && daysValue > 0 ? Date.now() - daysValue * 86_400_000 : 0,
      beforeMs: 0,
      exclude: splitList(exclude),
      toleranceMs: Math.max(0, Math.round((Number(toleranceSec) || 0) * 1000)),
    }
  }, [includeHiddenOn, exts, minMb, maxMb, days, exclude, toleranceSec])

  const cancelCounts = useCallback(() => {
    if (countsJob.current) {
      void toolCancel(countsJob.current).catch(() => undefined)
      countsJob.current = null
    }
  }, [])

  const cancelHash = useCallback(() => {
    if (hashJob.current) {
      void toolCancel(hashJob.current).catch(() => undefined)
      hashJob.current = null
    }
    setHashProgress(null)
  }, [])

  const patchStatus = useCallback((rel: string, status: CompareStatus) => {
    setCache((prev) => {
      const next = new Map(prev)
      for (const [level, entries] of next) {
        if (!entries.some((entry) => entry.rel === rel)) continue
        next.set(
          level,
          entries.map((entry) => (entry.rel === rel ? { ...entry, status } : entry)),
        )
        break
      }
      return next
    })
  }, [])

  const startHash = useCallback(
    (rel: string, entries: CompareEntry[], active: Session) => {
      if (!hashOn) return
      const pairs = entries
        .filter(
          (entry) =>
            entry.status === 'same' &&
            entry.left &&
            entry.right &&
            !entry.left.isDir &&
            !entry.right.isDir &&
            !hashResults[entry.rel],
        )
        .map((entry) => ({
          rel: entry.rel,
          left: joinPath(active.left, entry.rel),
          right: joinPath(active.right, entry.rel),
        }))
      if (!pairs.length) return
      cancelHash()
      const jobId = `cmp-hash-${++seq.current}`
      hashJob.current = jobId
      setHashProgress({
        jobId,
        current: 0,
        total: pairs.length,
        name: null,
        bytes: 0,
        bytesTotal: 0,
        paused: false,
        done: false,
        aborted: false,
        error: null,
        results: [],
      })
      compareHashStart(jobId, pairs, hashRateBytes, (event) => {
        if (hashJob.current !== jobId) return
        setHashProgress(event)
        if (event.results.length) {
          setHashResults((prev) => {
            const next = { ...prev }
            for (const result of event.results) next[result.rel] = result
            return next
          })
          for (const result of event.results) {
            if (!result.equal) patchStatus(result.rel, 'different')
          }
        }
        if (event.done) {
          if (hashJob.current === jobId) hashJob.current = null
          if (event.aborted) setHashProgress(null)
        }
      }).catch(() => {
        if (hashJob.current === jobId) hashJob.current = null
        setHashProgress(null)
      })
    },
    [cancelHash, hashOn, hashRateBytes, hashResults, patchStatus],
  )

  const startCounts = useCallback(
    (rel: string, entries: CompareEntry[], active: Session) => {
      const shared = entries.filter(
        (entry) => entry.left?.isDir && entry.right?.isDir,
      )
      if (!shared.length) return
      cancelCounts()
      const jobId = `cmp-count-${++seq.current}`
      countsJob.current = jobId
      compareCountsStart(jobId, active.left, active.right, rel, active.options, (event) => {
        setCounts((prev) => ({ ...prev, [event.name]: event }))
        if (event.done && countsJob.current === jobId) countsJob.current = null
      }).catch(() => {
        if (countsJob.current === jobId) countsJob.current = null
      })
    },
    [cancelCounts],
  )

  const loadLevel = useCallback(
    async (rel: string, active: Session, refresh: boolean) => {
      cancelCounts()
      cancelHash()
      setLoading(true)
      setError(null)
      try {
        const left = rel ? joinPath(active.left, rel) : active.left
        const right = rel ? joinPath(active.right, rel) : active.right
        const result = await compareLevel(left, right, rel, active.options)
        const entries = result.entries
        setCache((prev) => {
          if (!refresh && prev.has(rel)) return prev
          const next = new Map(prev)
          next.set(rel, entries)
          return next
        })
        setLevelRel(rel)
        startCounts(rel, entries, active)
        startHash(rel, entries, active)
      } catch (e: unknown) {
        setError(String(e))
      } finally {
        setLoading(false)
      }
    },
    [cancelCounts, cancelHash, startCounts, startHash],
  )

  /** New roots or filters invalidate every cached level and every count/hash result. */
  const begin = useCallback(() => {
    const left = leftRoot.trim()
    const right = rightRoot.trim()
    if (!left || !right || left.toLowerCase() === right.toLowerCase()) return
    cancelCounts()
    cancelHash()
    setCache(new Map())
    setCounts({})
    setHashResults({})
    setPlan(null)
    setPreviewOpen(false)
    const active: Session = { left, right, options }
    setSession(active)
    void loadLevel('', active, true)
  }, [cancelCounts, cancelHash, leftRoot, loadLevel, options, rightRoot])

  // Entering compare from two panes with folders already open should show a result
  // without a second click.
  useEffect(() => {
    if (leftRoot.trim() && rightRoot.trim()) begin()
    // Only on mount: later option edits wait for the explicit refresh button.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    return () => {
      if (countsJob.current) void toolCancel(countsJob.current).catch(() => undefined)
      if (hashJob.current) void toolCancel(hashJob.current).catch(() => undefined)
    }
  }, [])

  // Toggling hashes on after a level is already open should start verifying it rather
  // than wait for a refresh.
  useEffect(() => {
    if (hashOn && session) startHash(levelRel, cache.get(levelRel) ?? [], session)
    // Only on the switch: `startHash` already skips pairs it verified before.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hashOn])

  const entries = cache.get(levelRel) ?? []
  const visible = useMemo(
    () => entries.filter((entry) => statuses.has(entry.status)),
    [entries, statuses],
  )

  const breadcrumb = useMemo(() => {
    const segments = levelRel ? levelRel.split('\\') : []
    return segments.map((segment, index) => ({
      label: segment,
      rel: segments.slice(0, index + 1).join('\\'),
    }))
  }, [levelRel])

  const toggleStatus = (status: CompareStatus) => {
    setStatuses((prev) => {
      const next = new Set(prev)
      if (next.has(status)) next.delete(status)
      else next.add(status)
      return next
    })
  }

  const statusCounts = useMemo(() => {
    const result: Record<CompareStatus, number> = { left: 0, right: 0, different: 0, same: 0 }
    for (const entry of entries) result[entry.status] += 1
    return result
  }, [entries])

  const decide = (entry: CompareEntry): SyncAction | 'conflict' | 'mismatch' | null => {
    if (!session) return null
    const leftPath = joinPath(session.left, entry.rel)
    const rightPath = joinPath(session.right, entry.rel)
    const targetLeft = joinPath(session.left, parentRel(entry.rel))
    const targetRight = joinPath(session.right, parentRel(entry.rel))
    if (entry.left?.isDir || entry.right?.isDir) {
      if (entry.left && entry.right) {
        return entry.left.isDir !== entry.right.isDir ? 'mismatch' : null
      }
      if (entry.left && !entry.right) {
        if (strategy === 'r2l') return deleteExtras ? { kind: 'delete', paths: [leftPath] } : null
        return { kind: 'copy', source: leftPath, targetDir: targetRight }
      }
      if (entry.right && !entry.left) {
        if (strategy === 'l2r') return deleteExtras ? { kind: 'delete', paths: [rightPath] } : null
        return { kind: 'copy', source: rightPath, targetDir: targetLeft }
      }
      return 'mismatch'
    }
    if (entry.left && entry.right) {
      if (entry.status === 'same') return null
      if (strategy === 'l2r') return { kind: 'copy', source: leftPath, targetDir: targetRight }
      if (strategy === 'r2l') return { kind: 'copy', source: rightPath, targetDir: targetLeft }
      const tolerance = session.options.toleranceMs
      const diff = entry.left.mtime - entry.right.mtime
      if (Math.abs(diff) <= tolerance) return 'conflict'
      return diff > 0
        ? { kind: 'copy', source: leftPath, targetDir: targetRight }
        : { kind: 'copy', source: rightPath, targetDir: targetLeft }
    }
    if (entry.left) {
      if (strategy === 'r2l') return deleteExtras ? { kind: 'delete', paths: [leftPath] } : null
      return { kind: 'copy', source: leftPath, targetDir: targetRight }
    }
    if (strategy === 'l2r') return deleteExtras ? { kind: 'delete', paths: [rightPath] } : null
    return { kind: 'copy', source: rightPath, targetDir: targetLeft }
  }

  const openPreview = () => {
    const actions: SyncAction[] = []
    let conflicts = 0
    let mismatches = 0
    for (const list of cache.values()) {
      for (const entry of list) {
        const decision = decide(entry)
        if (decision === 'conflict') conflicts += 1
        else if (decision === 'mismatch') mismatches += 1
        else if (decision) actions.push(decision)
      }
    }
    setPlan({ actions, conflicts, mismatches })
    setPreviewOpen(true)
  }

  const execute = async () => {
    if (!plan || !session || executing) return
    if (!plan.actions.length) return
    if (
      !window.confirm(
        t('compare.confirmExecute', {
          count: plan.actions.reduce((sum, action) => sum + (action.kind === 'delete' ? action.paths.length : 1), 0),
        }),
      )
    ) {
      return
    }
    setExecuting(true)
    try {
      await onSync(plan.actions)
      setPreviewOpen(false)
      setPlan(null)
      setCounts({})
      setHashResults({})
      setCache(new Map())
      await loadLevel(levelRel, session, true)
      onToast(t('compare.done'))
    } finally {
      setExecuting(false)
    }
  }

  const exportCsv = async () => {
    if (exporting || !session) return
    setExporting(true)
    try {
      const dir = await pickFolder(t('tools.export'), session.left)
      if (!dir) return
      const rows = ['relative,status,name,leftSize,leftMtime,rightSize,rightMtime,hash']
      for (const list of cache.values()) {
        for (const entry of list) {
          const hash = hashResults[entry.rel]
          rows.push(
            [
              csvCell(entry.rel),
              csvCell(entry.status),
              csvCell(entry.name),
              csvCell(entry.left?.size ?? ''),
              csvCell(entry.left ? new Date(entry.left.mtime).toISOString() : ''),
              csvCell(entry.right?.size ?? ''),
              csvCell(entry.right ? new Date(entry.right.mtime).toISOString() : ''),
              csvCell(hash ? (hash.equal ? 'same' : 'different') : ''),
            ].join(','),
          )
        }
      }
      const stamp = new Date().toISOString().slice(0, 19).replace(/[:T]/g, '-')
      const written = await exportText(dir, `flowdir-compare-${stamp}.csv`, rows.join('\r\n'))
      onToast(t('tools.exported', { path: written }))
    } catch (e: unknown) {
      onToast(String(e))
    } finally {
      setExporting(false)
    }
  }

  const statusLabel = (status: CompareStatus) => {
    switch (status) {
      case 'left':
        return t('compare.status.left')
      case 'right':
        return t('compare.status.right')
      case 'different':
        return t('compare.status.different')
      default:
        return t('compare.status.same')
    }
  }
  const statusClass = (status: CompareStatus) => {
    switch (status) {
      case 'left':
        return 'text-rose-600 dark:text-rose-400'
      case 'right':
        return 'text-emerald-600 dark:text-emerald-400'
      case 'different':
        return 'text-amber-600 dark:text-amber-400'
      default:
        return 'text-muted-2'
    }
  }

  const fmtTime = (ms: number) =>
    ms ? new Date(ms).toLocaleString(lang === 'zh-CN' ? 'zh-CN' : 'en-US') : '—'

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/40 p-3 backdrop-blur-[1px]">
      <div className="flex max-h-[92vh] w-[72rem] max-w-full flex-col overflow-hidden rounded-xl border border-line bg-surface shadow-2xl">
        <div className="flex items-center gap-2 border-b border-line px-3 py-2">
          <span className="shrink-0 text-[0.8125rem] font-semibold text-fg-strong">{t('compare.title')}</span>
          <div className="flex min-w-0 flex-1 items-center gap-1.5">
            <input
              value={leftRoot}
              onChange={(e) => setLeftRoot(e.target.value)}
              aria-label={t('compare.left')}
              className="h-7 min-w-0 flex-1 rounded border border-line bg-surface-2 px-2 text-[0.6875rem] outline-none"
            />
            <span className="shrink-0 text-muted-2">⇄</span>
            <input
              value={rightRoot}
              onChange={(e) => setRightRoot(e.target.value)}
              aria-label={t('compare.right')}
              className="h-7 min-w-0 flex-1 rounded border border-line bg-surface-2 px-2 text-[0.6875rem] outline-none"
            />
          </div>
          <button
            type="button"
            onClick={begin}
            disabled={!leftRoot.trim() || !rightRoot.trim() || loading}
            className="shrink-0 rounded bg-blue-600 px-2.5 py-1 text-[0.6875rem] font-medium text-white hover:bg-blue-500 disabled:opacity-40"
          >
            {session ? t('compare.refresh') : t('compare.start')}
          </button>
          <button
            type="button"
            onClick={onClose}
            aria-label={t('compare.close')}
            className="grid h-6 w-6 shrink-0 place-items-center rounded text-muted hover:bg-surface-3 hover:text-fg"
          >
            ✕
          </button>
        </div>

        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-line px-3 py-1.5 text-[0.65625rem] text-muted-2">
          <button
            type="button"
            onClick={() => setShowOptions((open) => !open)}
            aria-expanded={showOptions}
            className="rounded border border-line px-1.5 py-0.5 hover:text-fg"
          >
            {t('compare.filters')}
          </button>
          <label className="flex items-center gap-1">
            <input type="checkbox" checked={hashOn} onChange={(e) => setHashOn(e.target.checked)} />
            {t('compare.hash')}
          </label>
          <label className="flex items-center gap-1">
            {t('compare.tolerance')}
            <input
              value={toleranceSec}
              onChange={(e) => setToleranceSec(e.target.value)}
              inputMode="decimal"
              className="h-6 w-10 rounded border border-line bg-surface-2 px-1 text-right text-fg outline-none"
            />
            {t('compare.seconds')}
          </label>
          <label className="flex items-center gap-1">
            <input type="checkbox" checked={includeHiddenOn} onChange={(e) => setIncludeHiddenOn(e.target.checked)} />
            {t('compare.includeHidden')}
          </label>
          {hashProgress && !hashProgress.done && (
            <span className="flex items-center gap-1 text-blue-500">
              {t('compare.verifying', { current: hashProgress.current, total: hashProgress.total })}
              <button type="button" onClick={cancelHash} className="rounded border border-line px-1 hover:text-fg">
                {t('compare.cancel')}
              </button>
            </span>
          )}
          {hashProgress?.done && (
            <span className="text-muted-2">
              {t('compare.verified', { current: hashProgress.current, total: hashProgress.total })}
            </span>
          )}
          <div className="ml-auto flex items-center gap-1">
            <button
              type="button"
              onClick={exportCsv}
              disabled={!cache.size || exporting}
              className="rounded border border-line px-1.5 py-0.5 hover:text-fg disabled:opacity-40"
            >
              {t('compare.export')}
            </button>
            <button
              type="button"
              onClick={openPreview}
              disabled={!cache.size}
              className="rounded border border-line px-1.5 py-0.5 hover:text-fg disabled:opacity-40"
            >
              {t('compare.preview')}
            </button>
          </div>
        </div>

        {showOptions && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 border-b border-line px-3 py-1.5 text-[0.65625rem] text-muted-2">
            <label className="flex items-center gap-1">
              {t('search.exts')}
              <input
                value={exts}
                onChange={(e) => setExts(e.target.value)}
                placeholder=".log .md"
                className="h-6 w-28 rounded border border-line bg-surface-2 px-1 text-fg outline-none"
              />
            </label>
            <label className="flex items-center gap-1">
              {t('search.minSize')}
              <input
                value={minMb}
                onChange={(e) => setMinMb(e.target.value)}
                inputMode="decimal"
                className="h-6 w-12 rounded border border-line bg-surface-2 px-1 text-right text-fg outline-none"
              />
              MB
            </label>
            <label className="flex items-center gap-1">
              {t('search.maxSize')}
              <input
                value={maxMb}
                onChange={(e) => setMaxMb(e.target.value)}
                inputMode="decimal"
                className="h-6 w-12 rounded border border-line bg-surface-2 px-1 text-right text-fg outline-none"
              />
              MB
            </label>
            <label className="flex items-center gap-1">
              {t('search.modified')}
              <input
                value={days}
                onChange={(e) => setDays(e.target.value)}
                inputMode="numeric"
                className="h-6 w-12 rounded border border-line bg-surface-2 px-1 text-right text-fg outline-none"
              />
              {t('compare.days')}
            </label>
            <label className="flex min-w-[12rem] flex-1 items-center gap-1">
              {t('compare.exclude')}
              <input
                value={exclude}
                onChange={(e) => setExclude(e.target.value)}
                placeholder="node_modules, *.tmp"
                className="h-6 min-w-0 flex-1 rounded border border-line bg-surface-2 px-1 text-fg outline-none"
              />
            </label>
            <span className="text-muted-2">{t('compare.filtersHint')}</span>
          </div>
        )}

        <div className="flex items-center gap-1.5 border-b border-line px-3 py-1 text-[0.65625rem]">
          <button
            type="button"
            onClick={() => void loadLevel('', session as Session, false)}
            disabled={!session}
            className="rounded px-1.5 py-0.5 text-muted hover:bg-surface-3 hover:text-fg"
          >
            {t('compare.root')}
          </button>
          {breadcrumb.map((crumb) => (
            <button
              key={crumb.rel}
              type="button"
              onClick={() => session && void loadLevel(crumb.rel, session, false)}
              className="truncate rounded px-1.5 py-0.5 text-muted hover:bg-surface-3 hover:text-fg"
            >
              ‹ {crumb.label}
            </button>
          ))}
          <div className="ml-auto flex items-center gap-1">
            {STATUS_ORDER.map((status) => (
              <button
                key={status}
                type="button"
                onClick={() => toggleStatus(status)}
                className={`rounded-full border px-1.5 py-0.5 ${
                  statuses.has(status)
                    ? `border-line bg-surface-3 ${statusClass(status)}`
                    : 'border-transparent text-muted-2'
                }`}
              >
                {statusLabel(status)} {statusCounts[status]}
              </button>
            ))}
          </div>
        </div>

        <div className="min-h-0 flex-1 overflow-auto">
          {loading && (
            <div className="px-3 py-6 text-center text-[0.71875rem] text-muted-2">{t('compare.loading')}</div>
          )}
          {error && !loading && (
            <div className="m-3 rounded border border-red-300 bg-red-50 p-2 text-[0.6875rem] text-red-800 dark:border-red-900 dark:bg-red-950/40 dark:text-red-200">
              {t('compare.error')} {error}
            </div>
          )}
          {!loading && !error && (
            <table className="w-full table-fixed border-collapse text-left text-[0.6875rem]">
              <thead className="sticky top-0 z-10 bg-surface-2 text-[0.625rem] text-muted-2">
                <tr>
                  <th className="w-[40%] px-2 py-1 font-normal">{t('compare.name')}</th>
                  <th className="w-[15%] px-2 py-1 text-right font-normal">{t('compare.leftSize')}</th>
                  <th className="w-[15%] px-2 py-1 text-right font-normal">{t('compare.rightSize')}</th>
                  <th className="w-[18%] px-2 py-1 font-normal">{t('compare.status')}</th>
                  <th className="w-[12%] px-2 py-1 text-right font-normal">{t('compare.counts')}</th>
                </tr>
              </thead>
              <tbody>
                {visible.map((entry) => {
                  const count = counts[entry.rel]
                  const hash = hashResults[entry.rel]
                  const canEnter = !!entry.left?.isDir && !!entry.right?.isDir
                  const rowClass =
                    entry.status === 'left'
                      ? 'bg-rose-50/50 dark:bg-rose-950/20'
                      : entry.status === 'right'
                        ? 'bg-emerald-50/50 dark:bg-emerald-950/20'
                        : entry.status === 'different'
                          ? 'bg-amber-50/50 dark:bg-amber-950/20'
                          : ''
                  return (
                    <tr
                      key={entry.rel}
                      className={`border-b border-row-line ${rowClass} hover:bg-surface-3`}
                      onDoubleClick={() => canEnter && session && void loadLevel(entry.rel, session, false)}
                    >
                      <td className="px-2 py-1">
                        <div className="flex min-w-0 items-center gap-1.5">
                          <FileIcon
                            name={entry.name}
                            dir={!!entry.left?.isDir || !!entry.right?.isDir}
                            path={joinPath(session?.left ?? '', entry.rel)}
                            scale={iconScale}
                          />
                          <span className="truncate text-fg-strong" title={entry.rel}>
                            {entry.name}
                          </span>
                          {canEnter && (
                            <button
                              type="button"
                              onClick={() => session && void loadLevel(entry.rel, session, false)}
                              className="shrink-0 rounded border border-line px-1 text-[0.625rem] text-muted hover:text-fg"
                            >
                              {t('compare.enter')}
                            </button>
                          )}
                        </div>
                      </td>
                      <td className="truncate px-2 py-1 text-right text-muted-2">
                        {entry.left ? `${formatSize(entry.left.size, entry.left.isDir) || '—'} · ${fmtTime(entry.left.mtime)}` : '—'}
                      </td>
                      <td className="truncate px-2 py-1 text-right text-muted-2">
                        {entry.right ? `${formatSize(entry.right.size, entry.right.isDir) || '—'} · ${fmtTime(entry.right.mtime)}` : '—'}
                      </td>
                      <td className={`px-2 py-1 ${statusClass(entry.status)}`}>
                        {statusLabel(entry.status)}
                        {hash && (
                          <span
                            title={`${hash.leftHash ?? ''} / ${hash.rightHash ?? ''}${hash.error ? ` — ${hash.error}` : ''}`}
                            className={hash.equal ? 'ml-1 text-emerald-500' : 'ml-1 text-rose-500'}
                          >
                            {hash.equal ? '=' : '≠'}
                          </span>
                        )}
                      </td>
                      <td className="truncate px-2 py-1 text-right text-muted-2">
                        {count ? (
                          <span title={t('compare.countsHint')}>
                            {count.leftOnly} / {count.different} / {count.rightOnly}
                          </span>
                        ) : canEnter ? (
                          <span title={t('compare.scanning')}>…</span>
                        ) : (
                          ''
                        )}
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          )}
          {!loading && !error && !visible.length && (
            <div className="px-3 py-8 text-center text-[0.71875rem] text-muted-2">
              {entries.length ? t('compare.filteredEmpty') : t('compare.empty')}
            </div>
          )}
        </div>

        <div className="flex items-center justify-between gap-2 border-t border-line px-3 py-1 text-[0.65625rem] text-muted-2">
          <span>{t('compare.levelHint')}</span>
          <span>{t('compare.loadedOnly')}</span>
        </div>
      </div>

      {previewOpen && plan && (
        <div className="fixed inset-0 z-[60] flex items-center justify-center bg-slate-950/40 p-4">
          <div className="flex max-h-[80vh] w-[46rem] flex-col overflow-hidden rounded-xl border border-line bg-surface shadow-2xl">
            <div className="flex items-center justify-between border-b border-line px-3 py-2">
              <span className="text-[0.8125rem] font-semibold text-fg-strong">{t('compare.previewTitle')}</span>
              <button
                type="button"
                onClick={() => setPreviewOpen(false)}
                aria-label={t('compare.close')}
                className="grid h-6 w-6 place-items-center rounded text-muted hover:bg-surface-3 hover:text-fg"
              >
                ✕
              </button>
            </div>
            <div className="flex flex-wrap items-center gap-2 border-b border-line px-3 py-1.5 text-[0.65625rem] text-muted-2">
              <label className="flex items-center gap-1">
                {t('compare.strategy')}
                <select
                  value={strategy}
                  onChange={(e) => {
                    setStrategy(e.target.value as 'l2r' | 'r2l' | 'both')
                    setPlan(null)
                    setPreviewOpen(false)
                  }}
                  className="h-6 rounded border border-line bg-surface-2 px-1 text-fg outline-none"
                >
                  <option value="l2r">{t('compare.strategyL2R')}</option>
                  <option value="r2l">{t('compare.strategyR2L')}</option>
                  <option value="both">{t('compare.strategyBoth')}</option>
                </select>
              </label>
              <label className="flex items-center gap-1">
                <input
                  type="checkbox"
                  checked={deleteExtras}
                  onChange={(e) => {
                    setDeleteExtras(e.target.checked)
                    setPlan(null)
                    setPreviewOpen(false)
                  }}
                />
                {t('compare.deleteExtra')}
              </label>
              <span className="ml-auto">
                {t('compare.actionCount', { count: plan.actions.length })}
                {plan.conflicts ? ` · ${t('compare.conflicts', { count: plan.conflicts })}` : ''}
                {plan.mismatches ? ` · ${t('compare.mismatches', { count: plan.mismatches })}` : ''}
              </span>
            </div>
            <div className="min-h-0 flex-1 overflow-auto">
              {plan.actions.length === 0 ? (
                <div className="px-3 py-8 text-center text-[0.71875rem] text-muted-2">{t('compare.previewEmpty')}</div>
              ) : (
                <ul className="divide-y divide-row-line text-[0.6875rem]">
                  {plan.actions.map((action, index) => (
                    <li key={index} className="flex items-center gap-2 px-3 py-1">
                      <span
                        className={`w-16 shrink-0 rounded px-1 text-center text-[0.625rem] ${
                          action.kind === 'delete'
                            ? 'bg-rose-100 text-rose-700 dark:bg-rose-950 dark:text-rose-300'
                            : 'bg-blue-100 text-blue-700 dark:bg-blue-950 dark:text-blue-300'
                        }`}
                      >
                        {action.kind === 'delete' ? t('compare.action.delete') : t('compare.action.copy')}
                      </span>
                      <span className="min-w-0 flex-1 truncate text-fg" title={action.kind === 'copy' ? action.source : action.paths[0]}>
                        {action.kind === 'copy' ? action.source : action.paths.join(', ')}
                      </span>
                      <span className="shrink-0 text-muted-2">
                        → {action.kind === 'copy' ? action.targetDir : t('compare.recycle')}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </div>
            <div className="flex items-center justify-between gap-2 border-t border-line px-3 py-2 text-[0.65625rem] text-muted-2">
              <span>{t('compare.loadedOnly')}</span>
              <div className="flex items-center gap-1">
                <button
                  type="button"
                  onClick={() => setPreviewOpen(false)}
                  className="rounded border border-line px-2 py-1 hover:text-fg"
                >
                  {t('compare.close')}
                </button>
                <button
                  type="button"
                  onClick={() => void execute()}
                  disabled={executing || !plan.actions.length}
                  className="rounded bg-blue-600 px-2.5 py-1 text-white hover:bg-blue-500 disabled:opacity-40"
                >
                  {executing ? t('compare.executing') : t('compare.execute')}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}