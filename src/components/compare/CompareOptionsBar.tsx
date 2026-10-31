import type { Translate } from '../../i18n'

export interface CompareOptionsBarProps {
  exts: string
  setExts: (v: string) => void
  minMb: string
  setMinMb: (v: string) => void
  maxMb: string
  setMaxMb: (v: string) => void
  days: string
  setDays: (v: string) => void
  exclude: string
  setExclude: (v: string) => void
  t: Translate
}

export function CompareOptionsBar({
  exts,
  setExts,
  minMb,
  setMinMb,
  maxMb,
  setMaxMb,
  days,
  setDays,
  exclude,
  setExclude,
  t,
}: CompareOptionsBarProps) {
  return (
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
  )
}
