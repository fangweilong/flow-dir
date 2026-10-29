import React, { useRef } from 'react'
import type { PointerEvent as ReactPointerEvent } from 'react'
import type { Node } from '../workspace'

export type SplitNode = Extract<Node, { t: 'split' }>

export const GUTTER = 2

export function ResizeHandle({
  onPointerDown,
}: {
  onPointerDown: (ev: ReactPointerEvent<HTMLDivElement>) => void
}) {
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      onPointerDown={onPointerDown}
      style={{ width: GUTTER }}
      className="shrink-0 cursor-col-resize bg-line-strong hover:bg-blue-400"
    />
  )
}

export function SplitView({
  node,
  render,
  onRatio,
}: {
  node: SplitNode
  render: (n: Node) => React.ReactNode
  onRatio: (splitId: string, ratio: number) => void
}) {
  const ref = useRef<HTMLDivElement>(null)
  return (
    <div ref={ref} className={`flex min-h-0 min-w-0 flex-1 ${node.dir === 'row' ? 'flex-row' : 'flex-col'}`}>
      <div className="flex min-h-0 min-w-0" style={{ flexGrow: node.ratio, flexBasis: 0 }}>
        {render(node.a)}
      </div>
      <div
        onPointerDown={(ev) => {
          ev.preventDefault()
          const container = ref.current
          if (!container) return
          const move = (e: PointerEvent) => {
            const rect = container.getBoundingClientRect()
            const raw =
              node.dir === 'row' ? (e.clientX - rect.left) / rect.width : (e.clientY - rect.top) / rect.height
            onRatio(node.id, Math.min(0.85, Math.max(0.15, raw)))
          }
          const up = () => {
            window.removeEventListener('pointermove', move)
            window.removeEventListener('pointerup', up)
          }
          window.addEventListener('pointermove', move)
          window.addEventListener('pointerup', up)
        }}
        className={`shrink-0 bg-line-strong hover:bg-blue-400 ${
          node.dir === 'row' ? 'w-0.5 cursor-col-resize' : 'h-0.5 cursor-row-resize'
        }`}
      />
      <div className="flex min-h-0 min-w-0" style={{ flexGrow: 1 - node.ratio, flexBasis: 0 }}>
        {render(node.b)}
      </div>
    </div>
  )
}
