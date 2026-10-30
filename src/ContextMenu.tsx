import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react'

export type MenuItem =
  | {
      kind: 'item'
      label: string
      hint?: string
      disabled?: boolean
      danger?: boolean
      /** FlowDir's own commands carry a mark; menus holding any icon reserve the gutter. */
      icon?: ReactNode
      onSelect: () => void
    }
  | { kind: 'sep' }

type Props = {
  x: number
  y: number
  items: MenuItem[]
  onClose: () => void
}

/** Self-drawn menu for tabs, breadcrumbs, the drive overview and native-menu fallbacks. */
export function ContextMenu({ x, y, items, onClose }: Props) {
  const ref = useRef<HTMLDivElement>(null)
  const [pos, setPos] = useState({ x, y })
  /** Enabled entries in DOM order; arrow keys walk this list, skips separators. */
  const entries = () =>
    Array.from(ref.current?.querySelectorAll<HTMLButtonElement>('[data-menu-item]:not(:disabled)') ?? [])
  const cleanItems = items.filter((item, index, arr) => {
    if (item.kind !== 'sep') return true
    if (index === 0 || index === arr.length - 1) return false
    return arr[index - 1]?.kind !== 'sep'
  })
  /** Icons share one left gutter; labels without a mark keep the same indent. */
  const hasIcons = cleanItems.some((item) => item.kind === 'item' && item.icon)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const rect = el.getBoundingClientRect()
    const nx = Math.min(x, window.innerWidth - rect.width - 4)
    const ny = Math.min(y, window.innerHeight - rect.height - 4)
    setPos({ x: Math.max(4, nx), y: Math.max(4, ny) })
    // Keyboard users open the menu with Shift+F10; without this the first arrow key
    // would leave the menu instead of moving inside it.
    entries()[0]?.focus()
  }, [x, y])

  useEffect(() => {
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === 'Escape') {
        ev.preventDefault()
        onClose()
        return
      }
      if (ev.key !== 'ArrowDown' && ev.key !== 'ArrowUp' && ev.key !== 'Home' && ev.key !== 'End') return
      const list = entries()
      if (!list.length) return
      ev.preventDefault()
      const index = list.indexOf(document.activeElement as HTMLButtonElement)
      const next =
        ev.key === 'Home'
          ? 0
          : ev.key === 'End'
            ? list.length - 1
            : ev.key === 'ArrowDown'
              ? (index + 1) % list.length
              : (index <= 0 ? list.length : index) - 1
      list[next].focus()
    }
    const onDown = (ev: MouseEvent) => {
      if (ref.current && !ref.current.contains(ev.target as Node)) onClose()
    }
    window.addEventListener('keydown', onKey)
    window.addEventListener('mousedown', onDown)
    window.addEventListener('blur', onClose)
    return () => {
      window.removeEventListener('keydown', onKey)
      window.removeEventListener('mousedown', onDown)
      window.removeEventListener('blur', onClose)
    }
  }, [onClose])

  return (
    <div
      ref={ref}
      data-context-menu
      role="menu"
      tabIndex={-1}
      className="fixed z-[60] min-w-[11.5rem] rounded-lg border border-line bg-surface py-1 shadow-xl"
      style={{ left: pos.x, top: pos.y }}
      onContextMenu={(ev) => ev.preventDefault()}
    >
      {cleanItems.map((item, index) =>
        item.kind === 'sep' ? (
          <div key={`sep-${index}`} role="separator" className="my-1 h-px bg-line" />
        ) : (
          <button
            key={`${item.label}-${index}`}
            type="button"
            role="menuitem"
            data-menu-item
            disabled={item.disabled}
            onClick={() => {
              onClose()
              item.onSelect()
            }}
            className={`flex w-full items-center justify-between gap-6 px-3 py-1 text-left text-[0.75rem] disabled:opacity-40 ${
              item.danger ? 'text-red-600 hover:bg-red-500/10 dark:text-red-400' : 'text-fg hover:bg-surface-3'
            }`}
          >
            <span className="flex min-w-0 items-center gap-2">
              {hasIcons && <span className="grid w-[1.1em] shrink-0 place-items-center">{item.icon}</span>}
              <span className="truncate">{item.label}</span>
            </span>
            {item.hint && <span className="shrink-0 font-mono text-[0.625rem] text-muted-2">{item.hint}</span>}
          </button>
        ),
      )}
    </div>
  )
}
