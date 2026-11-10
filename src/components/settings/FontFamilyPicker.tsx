import { useEffect, useId, useMemo, useRef, useState } from 'react'
import type { KeyboardEvent as ReactKeyboardEvent } from 'react'
import { fontStack } from '../../fonts'

/** Enough rows to cover a typed filter without building hundreds of nodes per keystroke. */
const MAX_MATCHES = 80

export type FontFamilyPickerProps = {
  value: string
  /** Installed families plus the curated defaults; the picker only suggests these. */
  options: string[]
  onChange: (family: string) => void
  ariaLabel: string
  placeholder?: string
  /** Region overrides accept an empty value, meaning "inherit the global family". */
  allowEmpty?: boolean
  emptyLabel?: string
  noMatchLabel?: string
  className?: string
}

/**
 * Searchable family picker: the input accepts free text (settings still validate the
 * stored value), while the list offers every installed family filtered as you type.
 * Typing commits immediately so the font change previews live; Escape reverts it.
 */
export function FontFamilyPicker({
  value,
  options,
  onChange,
  ariaLabel,
  placeholder,
  allowEmpty,
  emptyLabel,
  noMatchLabel,
  className,
}: FontFamilyPickerProps) {
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [active, setActive] = useState(0)
  const rootRef = useRef<HTMLDivElement>(null)
  const listRef = useRef<HTMLUListElement>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const valueAtOpen = useRef(value)
  const listId = useId()

  useEffect(() => {
    if (!open) return
    const onPointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false)
    }
    window.addEventListener('pointerdown', onPointerDown)
    return () => window.removeEventListener('pointerdown', onPointerDown)
  }, [open])

  const entries = useMemo(() => {
    const needle = query.trim().toLowerCase()
    const matched = (needle ? options.filter((name) => name.toLowerCase().includes(needle)) : options).slice(
      0,
      MAX_MATCHES,
    )
    const fonts = matched.map((name) => ({ family: name, label: name }))
    return allowEmpty ? [{ family: '', label: emptyLabel ?? '' }, ...fonts] : fonts
  }, [allowEmpty, emptyLabel, options, query])

  useEffect(() => {
    if (!open) return
    listRef.current
      ?.querySelector<HTMLElement>(`[data-index="${active}"]`)
      ?.scrollIntoView({ block: 'nearest' })
  }, [active, open])

  const show = () => {
    valueAtOpen.current = value
    setQuery('')
    setActive(0)
    setOpen(true)
  }

  const close = (restore: boolean) => {
    if (restore) onChange(valueAtOpen.current)
    setQuery('')
    setOpen(false)
  }

  const commit = (family: string) => {
    onChange(family)
    setQuery('')
    setOpen(false)
  }

  const onKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      if (!open) {
        show()
        return
      }
      const step = event.key === 'ArrowDown' ? 1 : -1
      setActive((index) =>
        Math.max(0, Math.min(entries.length - 1, index + step)),
      )
      return
    }
    if (event.key === 'Enter') {
      if (!open) return
      event.preventDefault()
      const entry = entries[active]
      if (entry) commit(entry.family)
      else setOpen(false)
      return
    }
    if (event.key === 'Escape') {
      if (!open) return
      event.preventDefault()
      // The settings modal closes on Escape; the picker consumes it while its list is open.
      event.stopPropagation()
      close(true)
      return
    }
    if (event.key === 'Tab') setOpen(false)
  }

  return (
    <div ref={rootRef} className={`relative ${className ?? ''}`}>
      <input
        ref={inputRef}
        role="combobox"
        aria-label={ariaLabel}
        aria-expanded={open}
        aria-controls={listId}
        aria-autocomplete="list"
        aria-activedescendant={open && entries[active] ? `${listId}-${active}` : undefined}
        value={value}
        placeholder={placeholder}
        spellCheck={false}
        autoComplete="off"
        onChange={(event) => {
          const next = event.target.value
          onChange(next)
          if (!open) {
            // Typing without a preceding focus event still starts a fresh edit.
            valueAtOpen.current = value
            setOpen(true)
          }
          setQuery(next)
          setActive(0)
        }}
        onFocus={(event) => {
          const input = event.currentTarget
          if (!open) show()
          // Select the whole family so typing searches instead of appending to it.
          requestAnimationFrame(() => input.select())
        }}
        onKeyDown={onKeyDown}
        className="h-6 w-full rounded border border-line bg-surface-2 pr-6 pl-1.5 text-[0.71875rem] text-fg outline-none placeholder:text-muted-2 focus:border-accent-soft focus:bg-surface"
      />
      <button
        type="button"
        tabIndex={-1}
        aria-label={ariaLabel}
        aria-expanded={open}
        onMouseDown={(event) => event.preventDefault()}
        onClick={() => {
          if (open) {
            close(false)
            return
          }
          inputRef.current?.focus()
          show()
        }}
        className="absolute inset-y-0 right-0 flex w-6 items-center justify-center rounded-r text-muted-2 hover:text-fg"
      >
        <svg viewBox="0 0 10 10" aria-hidden="true" className="h-2.5 w-2.5 fill-current">
          <path d="M1 3l4 4 4-4z" />
        </svg>
      </button>
      {open && (
        <ul
          ref={listRef}
          id={listId}
          role="listbox"
          aria-label={ariaLabel}
          className="absolute right-0 top-full z-50 mt-1 max-h-60 w-full min-w-[12rem] overflow-y-auto rounded-md border border-line bg-surface py-0.5 shadow-lg"
        >
          {entries.length === 0 && noMatchLabel && (
            <li className="px-2 py-1 text-[0.6875rem] text-muted-2">{noMatchLabel}</li>
          )}
          {entries.map((entry, index) => (
            <li
              key={entry.family || '__inherit'}
              id={`${listId}-${index}`}
              data-index={index}
              role="option"
              aria-selected={entry.family === value}
              onMouseDown={(event) => {
                event.preventDefault()
                commit(entry.family)
              }}
              onMouseEnter={() => setActive(index)}
              style={entry.family ? { fontFamily: fontStack(entry.family) } : undefined}
              className={`cursor-pointer truncate px-2 py-1 text-[0.71875rem] ${
                index === active ? 'bg-surface-3 text-fg-strong' : 'text-fg'
              }`}
            >
              {entry.label}
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
