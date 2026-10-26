import { useEffect, useLayoutEffect, useRef, useState } from 'react'

export type MenuItem =
  | { kind: 'item'; label: string; hint?: string; disabled?: boolean; danger?: boolean; onSelect: () => void }
  | { kind: 'sep' }

type Props = {
  x: number
  y: number
  items: MenuItem[]
  onClose: () => void
}

/** Self-drawn menu used for blank areas, tabs and breadcrumbs (files use the native shell menu). */
export function ContextMenu({ x, y, items, onClose }: Props) {
  const ref = useRef<HTMLDivElement>(null)
  const [pos, setPos] = useState({ x, y })

  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const rect = el.getBoundingClientRect()
    const nx = Math.min(x, window.innerWidth - rect.width - 4)
    const ny = Math.min(y, window.innerHeight - rect.height - 4)
    setPos({ x: Math.max(4, nx), y: Math.max(4, ny) })
  }, [x, y])

  useEffect(() => {
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === 'Escape') onClose()
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
      className="fixed z-[60] min-w-[11.5rem] rounded-lg border border-line bg-surface py-1 shadow-xl"
      style={{ left: pos.x, top: pos.y }}
      onContextMenu={(ev) => ev.preventDefault()}
    >
      {items.map((item, index) =>
        item.kind === 'sep' ? (
          <div key={`sep-${index}`} className="my-1 h-px bg-line" />
        ) : (
          <button
            key={item.label}
            type="button"
            disabled={item.disabled}
            onClick={() => {
              onClose()
              item.onSelect()
            }}
            className={`flex w-full items-center justify-between gap-6 px-3 py-1 text-left text-[0.75rem] disabled:opacity-40 ${
              item.danger ? 'text-red-600 hover:bg-red-500/10 dark:text-red-400' : 'text-fg hover:bg-surface-3'
            }`}
          >
            <span className="truncate">{item.label}</span>
            {item.hint && <span className="shrink-0 font-mono text-[0.625rem] text-muted-2">{item.hint}</span>}
          </button>
        ),
      )}
    </div>
  )
}
