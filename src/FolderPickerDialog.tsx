import { useEffect, useRef, useState } from 'react'
import { drives, treeDirs } from './api'
import { makeT } from './i18n'
import { joinPath, parentPath } from './paths'
import type { Drive, Lang } from './types'

type Props = {
  title: string
  initial: string
  lang: Lang
  /** Called with the folder the user confirmed; the dialog stays open on cancel. */
  onPick: (path: string) => void
  onClose: () => void
}

/**
 * In-app folder chooser: drives on top, one level of folders at a time. The shell's own
 * picker is a separate process and cannot be themed or automated, which is why the
 * extraction flow uses this one.
 */
export function FolderPickerDialog({ title, initial, lang, onPick, onClose }: Props) {
  const t = makeT(lang)
  const [path, setPath] = useState(initial)
  const [roots, setRoots] = useState<Drive[]>([])
  const [names, setNames] = useState<string[]>([])
  const [truncated, setTruncated] = useState(false)
  const [loading, setLoading] = useState(false)
  const pickRef = useRef<HTMLButtonElement>(null)

  useEffect(() => {
    pickRef.current?.focus()
  }, [])

  useEffect(() => {
    let alive = true
    void drives()
      .then((list) => {
        if (alive) setRoots(list.filter((drive) => drive.available))
      })
      .catch(() => undefined)
    return () => {
      alive = false
    }
  }, [])

  useEffect(() => {
    let alive = true
    setLoading(true)
    treeDirs(path, false)
      .then((result) => {
        if (!alive) return
        setNames(result.names)
        setTruncated(result.truncated)
      })
      .catch(() => {
        if (!alive) return
        setNames([])
        setTruncated(false)
      })
      .finally(() => {
        if (alive) setLoading(false)
      })
    return () => {
      alive = false
    }
  }, [path])

  useEffect(() => {
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === 'Escape') {
        ev.preventDefault()
        ev.stopPropagation()
        onClose()
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [onClose])

  const up = parentPath(path)
  const canGoUp = up && up !== path

  return (
    <div
      data-folder-picker
      className="fixed inset-0 z-[75] flex items-center justify-center bg-slate-950/30 p-4 backdrop-blur-[1px]"
      onMouseDown={(ev) => {
        if (ev.target === ev.currentTarget) onClose()
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className="flex w-[30rem] flex-col rounded-lg border border-line bg-surface p-3 shadow-2xl"
      >
        <div className="pb-1.5 text-[0.75rem] font-semibold text-fg-strong">{title}</div>
        <div className="mb-1.5 flex flex-wrap gap-1">
          {roots.map((drive) => (
            <button
              key={drive.root}
              type="button"
              title={drive.label || drive.name}
              onClick={() => setPath(drive.root)}
              className={`rounded border px-1.5 py-0.5 text-[0.65625rem] ${
                path === drive.root
                  ? 'border-blue-500 text-fg-strong'
                  : 'border-line text-muted hover:bg-surface-3 hover:text-fg'
              }`}
            >
              {drive.name}
            </button>
          ))}
        </div>
        <div
          data-picker-path
          className="mb-1.5 truncate rounded border border-line bg-surface-2 px-2 py-1 font-mono text-[0.65625rem] text-muted"
          title={path}
        >
          {path}
        </div>
        <div className="h-52 overflow-auto rounded border border-line">
          {canGoUp && (
            <button
              type="button"
              onClick={() => setPath(up)}
              className="flex w-full items-center gap-1.5 px-2 py-1 text-left text-[0.71875rem] text-muted hover:bg-surface-3 hover:text-fg"
            >
              ↑ {t('picker.up')}
            </button>
          )}
          {names.map((name) => (
            <button
              key={name}
              type="button"
              data-picker-dir={name}
              onClick={() => setPath(joinPath(path, name))}
              className="flex w-full items-center gap-1.5 px-2 py-1 text-left text-[0.71875rem] text-fg hover:bg-surface-3"
            >
              <span className="truncate">{name}</span>
            </button>
          ))}
          {!loading && !names.length && (
            <div className="px-2 py-1 text-[0.6875rem] text-muted-2">{t('picker.empty')}</div>
          )}
          {truncated && (
            <div className="px-2 py-1 text-[0.65625rem] text-muted-2">{t('picker.truncated')}</div>
          )}
        </div>
        <div className="flex justify-end gap-2 pt-3">
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-line px-3 py-1 text-[0.71875rem] text-fg hover:bg-surface-3"
          >
            {t('dialog.cancel')}
          </button>
          <button
            ref={pickRef}
            type="button"
            data-picker-ok
            onClick={() => onPick(path)}
            className="rounded-md bg-blue-600 px-3 py-1 text-[0.71875rem] font-medium text-white hover:bg-blue-500"
          >
            {t('picker.choose')}
          </button>
        </div>
      </div>
    </div>
  )
}
