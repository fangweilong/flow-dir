import { useEffect, useRef } from 'react'
import type { OpProgress } from '../types'
import { taskbarProgress } from '../api'

/** Mirrors aggregate operation progress onto the taskbar, coalesced to one update per frame-ish. */
export function useTaskbarProgress(ops: Record<string, OpProgress>) {
  const want = useRef<{ state: string; done: number; total: number } | null>(null)
  const timer = useRef<number | null>(null)
  const sentAt = useRef(0)

  useEffect(() => {
    const active = Object.values(ops).filter((op) => !op.done)
    const state = !active.length
      ? 'none'
      : active.some((op) => op.failedCount > 0 || op.error)
        ? 'error'
        : active.some((op) => op.aborted)
          ? 'paused'
          : 'normal'
    let done = 0
    let total = 0
    for (const op of active) {
      const each = Math.max(op.total, 1)
      done += Math.min(op.current, each)
      total += each
    }
    want.current = { state, done, total }
    const flush = () => {
      timer.current = null
      const pending = want.current
      if (!pending) return
      sentAt.current = Date.now()
      void taskbarProgress(pending.state, pending.done, pending.total).catch(() => undefined)
    }
    if (timer.current) return
    const wait = 160 - (Date.now() - sentAt.current)
    if (wait <= 0) flush()
    else timer.current = window.setTimeout(flush, wait)
  }, [ops])
}
