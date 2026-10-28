import { useEffect, useMemo, useRef, useState } from 'react'
import { makeT } from './i18n'
import { baseName, joinPath } from './paths'
import { pathsExist, uniquePaths } from './api'
import type { Lang } from './types'

type Props = {
  /** Folder the selected rows live in; every target path is resolved against it. */
  dir: string
  names: string[]
  lang: Lang
  onConfirm: (pairs: { from: string; to: string }[]) => void
  onClose: () => void
}

type Mode = 'replace' | 'sequence'

/** How a name already present on disk is handled. `ask` blocks until the user picks one. */
type ConflictPolicy = 'ask' | 'skip' | 'number'

const RESERVED = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])$/i
const ILLEGAL = /[\u0000-\u001f<>:"/\\|?*]/

function splitName(name: string): { stem: string; ext: string } {
  const dot = name.lastIndexOf('.')
  return dot > 0 ? { stem: name.slice(0, dot), ext: name.slice(dot) } : { stem: name, ext: '' }
}

function invalidName(name: string): boolean {
  const { stem } = splitName(name)
  return (
    !name ||
    ILLEGAL.test(name) ||
    /[ .]$/.test(name) ||
    name === '.' ||
    name === '..' ||
    RESERVED.test(stem)
  )
}

/**
 * Batch rename preview: both modes are pure functions of the current names, so the exact
 * list that will be submitted is visible before anything touches the filesystem.
 */
export function BatchRenameDialog({ dir, names, lang, onConfirm, onClose }: Props) {
  const t = makeT(lang)
  const [mode, setMode] = useState<Mode>('replace')
  const [find, setFind] = useState('')
  const [replace, setReplace] = useState('')
  const [template, setTemplate] = useState('{name}_{n}')
  const [start, setStart] = useState(1)
  const [digits, setDigits] = useState(2)
  const [policy, setPolicy] = useState<ConflictPolicy>('ask')
  /** Rows whose target name exists on disk and would overwrite an unrelated entry. */
  const [conflicts, setConflicts] = useState<Set<number>>(new Set())
  const findRef = useRef<HTMLInputElement>(null)
  const templateRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (mode === 'replace') findRef.current?.focus()
    else templateRef.current?.focus()
  }, [mode])

  const plan = useMemo(
    () =>
      names.map((name, index) => {
        const { stem, ext } = splitName(name)
        const nextStem =
          mode === 'replace'
            ? find
              ? stem.split(find).join(replace)
              : stem
            : template
                .replaceAll('{name}', stem)
                .replaceAll('{n}', String(start + index).padStart(digits, '0'))
        return { from: name, to: `${nextStem}${ext}` }
      }),
    [names, mode, find, replace, template, start, digits],
  )

  const invalid = useMemo(() => {
    const rows = new Set<number>()
    const seen = new Map<string, number>()
    plan.forEach((pair, index) => {
      if (invalidName(pair.to)) rows.add(index)
      const key = pair.to.toLowerCase()
      const first = seen.get(key)
      if (first !== undefined) {
        rows.add(first)
        rows.add(index)
      } else {
        seen.set(key, index)
      }
    })
    return rows
  }, [plan])

  const effective = useMemo(
    () => plan.map((pair, index) => ({ pair, index })).filter((item) => item.pair.from !== item.pair.to),
    [plan],
  )
  /** Targets that differ from their source only in case are legal and never "conflicts". */
  const changedTargets = useMemo(
    () => effective.filter((item) => item.pair.from.toLowerCase() !== item.pair.to.toLowerCase()),
    [effective],
  )

  /**
   * Disk check, run whenever the preview changes. A hit only counts when the existing name
   * is neither the source itself nor another selected row that is also being renamed away:
   * a swap (`a→b`, `b→a`) must not be reported as an overwrite.
   */
  useEffect(() => {
    let stale = false
    if (!changedTargets.length) {
      setConflicts(new Set())
      return
    }
    const selected = new Set(names.map((name) => name.toLowerCase()))
    const renamedAway = new Set(
      plan.filter((pair) => pair.from !== pair.to).map((pair) => pair.from.toLowerCase()),
    )
    void pathsExist(changedTargets.map((item) => joinPath(dir, item.pair.to))).then((flags) => {
      if (stale) return
      const hit = new Set<number>()
      flags.forEach((exists, i) => {
        if (!exists) return
        const { index } = changedTargets[i]
        const target = plan[index].to.toLowerCase()
        const source = plan[index].from.toLowerCase()
        const freeByMove = selected.has(target) && renamedAway.has(target) && target !== source
        if (!freeByMove) hit.add(index)
      })
      setConflicts(hit)
    })
    return () => {
      stale = true
    }
  }, [changedTargets, plan, names, dir])

  const blocked = invalid.size > 0 || effective.length === 0
  const needsDecision = conflicts.size > 0 && policy === 'ask'

  const submit = () => {
    if (blocked || needsDecision) return
    if (!conflicts.size) {
      onConfirm(effective.map((item) => item.pair))
      return
    }
    if (policy === 'skip') {
      onConfirm(effective.filter((item) => !conflicts.has(item.index)).map((item) => item.pair))
      return
    }
    // `number`: ask the backend for the first free `name (n)` variant of every target,
    // including the ones that were already free.
    void uniquePaths(effective.map((item) => joinPath(dir, item.pair.to)))
      .then((resolved) => {
        onConfirm(
          effective.map((item, i) => ({ from: item.pair.from, to: baseName(resolved[i] ?? item.pair.to) })),
        )
      })
      .catch(() => onConfirm(effective.filter((item) => !conflicts.has(item.index)).map((item) => item.pair)))
  }

  return (
    <div
      data-batch-rename
      className="fixed inset-0 z-[70] flex items-start justify-center bg-slate-950/30 pt-[14vh] backdrop-blur-[1px]"
      onMouseDown={(ev) => {
        if (ev.target === ev.currentTarget) onClose()
      }}
    >
      <div
        onKeyDown={(ev) => {
          if (ev.nativeEvent.isComposing) return
          if (ev.key === 'Escape') {
            ev.preventDefault()
            onClose()
            return
          }
          if (ev.key === 'Enter') {
            // A focused button already has Enter semantics (Cancel, mode switch, …);
            // hijacking it here would submit the rename the user did not confirm.
            if (ev.target instanceof Element && ev.target.closest('button')) return
            ev.preventDefault()
            submit()
          }
        }}
        className="w-[34rem] max-w-[calc(100vw-1.5rem)] rounded-lg border border-line bg-surface p-3 shadow-2xl"
      >
        <div className="flex items-center justify-between pb-2">
          <span className="text-[0.75rem] font-semibold text-fg-strong">
            {t('batch.title', { count: names.length })}
          </span>
          <div className="flex rounded-md border border-line p-0.5 text-[0.6875rem]">
            {(['replace', 'sequence'] as Mode[]).map((entry) => (
              <button
                key={entry}
                type="button"
                onClick={() => setMode(entry)}
                className={`rounded px-2 py-0.5 ${
                  mode === entry ? 'bg-blue-600 text-white' : 'text-muted hover:bg-surface-3'
                }`}
              >
                {t(entry === 'replace' ? 'batch.modeReplace' : 'batch.modeSequence')}
              </button>
            ))}
          </div>
        </div>

        {mode === 'replace' ? (
          <div className="flex items-center gap-1.5 pb-2">
            <input
              ref={findRef}
              value={find}
              onChange={(ev) => setFind(ev.target.value)}
              placeholder={t('batch.find')}
              aria-label={t('batch.find')}
              className="h-7 min-w-0 flex-1 rounded border border-line bg-surface-2 px-2 text-[0.71875rem] outline-none placeholder:text-muted-2 focus:border-blue-400"
            />
            <span className="shrink-0 text-[0.6875rem] text-muted-2">→</span>
            <input
              value={replace}
              onChange={(ev) => setReplace(ev.target.value)}
              placeholder={t('batch.replace')}
              aria-label={t('batch.replace')}
              className="h-7 min-w-0 flex-1 rounded border border-line bg-surface-2 px-2 text-[0.71875rem] outline-none placeholder:text-muted-2 focus:border-blue-400"
            />
          </div>
        ) : (
          <div className="flex items-center gap-1.5 pb-2">
            <input
              ref={templateRef}
              value={template}
              onChange={(ev) => setTemplate(ev.target.value)}
              placeholder="{name}_{n}"
              aria-label={t('batch.template')}
              className="h-7 min-w-0 flex-1 rounded border border-line bg-surface-2 px-2 font-mono text-[0.71875rem] outline-none focus:border-blue-400"
            />
            <label className="flex shrink-0 items-center gap-1 text-[0.6875rem] text-muted-2">
              {t('batch.start')}
              <input
                type="number"
                min={0}
                max={9999}
                value={start}
                onChange={(ev) => setStart(Math.max(0, Math.min(9999, Number(ev.target.value) || 0)))}
                className="h-7 w-14 rounded border border-line bg-surface-2 px-1 text-[0.71875rem] text-fg outline-none focus:border-blue-400"
              />
            </label>
            <label className="flex shrink-0 items-center gap-1 text-[0.6875rem] text-muted-2">
              {t('batch.digits')}
              <input
                type="number"
                min={1}
                max={6}
                value={digits}
                onChange={(ev) => setDigits(Math.max(1, Math.min(6, Number(ev.target.value) || 1)))}
                className="h-7 w-12 rounded border border-line bg-surface-2 px-1 text-[0.71875rem] text-fg outline-none focus:border-blue-400"
              />
            </label>
          </div>
        )}

        <div className="pb-1 text-[0.65625rem] text-muted-2">{t('batch.preview')}</div>
        <div className="max-h-56 overflow-auto rounded border border-line bg-surface-2/60">
          {plan.map((pair, index) => (
            <div
              key={`${pair.from}-${index}`}
              data-batch-row={pair.from}
              data-batch-conflict={conflicts.has(index) ? '' : undefined}
              className={`grid grid-cols-[1fr_auto_1fr] items-center gap-2 border-b border-row-line px-2 py-1 text-[0.6875rem] last:border-b-0 ${
                invalid.has(index)
                  ? 'text-red-600 dark:text-red-300'
                  : conflicts.has(index)
                    ? 'text-amber-600 dark:text-amber-300'
                    : 'text-fg'
              }`}
            >
              <span className="truncate" title={pair.from}>
                {pair.from}
              </span>
              <span className="text-muted-2">→</span>
              <span
                className={`truncate ${pair.from === pair.to ? 'text-muted-2' : ''}`}
                title={conflicts.has(index) ? t('batch.conflictHint', { name: pair.to }) : pair.to}
              >
                {pair.to}
                {conflicts.has(index) && <span className="pl-1 text-[0.625rem]">⚠</span>}
              </span>
            </div>
          ))}
        </div>

        {conflicts.size > 0 && (
          <div
            data-batch-conflicts
            className="mt-2 rounded-md border border-amber-500/40 bg-amber-500/10 px-2 py-1.5"
          >
            <div className="pb-1 text-[0.6875rem] text-amber-700 dark:text-amber-300">
              {t('batch.conflictCount', { count: conflicts.size })}
            </div>
            <div className="flex flex-wrap gap-1">
              {(['skip', 'number'] as const).map((option) => (
                <button
                  key={option}
                  type="button"
                  data-batch-policy={option}
                  onClick={() => setPolicy(option)}
                  className={`rounded border px-2 py-0.5 text-[0.6875rem] ${
                    policy === option
                      ? 'border-blue-500 bg-blue-600 text-white'
                      : 'border-line text-fg hover:bg-surface-3'
                  }`}
                >
                  {t(option === 'skip' ? 'batch.conflictSkip' : 'batch.conflictNumber')}
                </button>
              ))}
              <button
                type="button"
                onClick={onClose}
                className="rounded border border-line px-2 py-0.5 text-[0.6875rem] text-fg hover:bg-surface-3"
              >
                {t('dialog.cancel')}
              </button>
            </div>
          </div>
        )}

        <div className="flex items-center justify-between gap-2 pt-2">
          <span className="min-w-0 flex-1 truncate text-[0.65625rem] text-muted-2">
            {invalid.size > 0
              ? t('batch.invalid')
              : needsDecision
                ? t('batch.conflictBlocked')
                : effective.length === 0
                  ? t('batch.unchanged')
                  : names.length > effective.length
                    ? t('batch.skipped', { count: names.length - effective.length })
                    : ''}
          </span>
          <button
            type="button"
            onClick={onClose}
            className="shrink-0 rounded-md border border-line px-3 py-1 text-[0.71875rem] text-fg hover:bg-surface-3"
          >
            {t('dialog.cancel')}
          </button>
          <button
            type="button"
            onClick={submit}
            disabled={blocked || needsDecision}
            className="shrink-0 rounded-md bg-blue-600 px-3 py-1 text-[0.71875rem] font-medium text-white hover:bg-blue-500 disabled:opacity-40"
          >
            {t('batch.confirm')}
          </button>
        </div>
      </div>
    </div>
  )
}
