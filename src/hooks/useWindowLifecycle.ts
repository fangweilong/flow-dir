import { useEffect, useState } from 'react'
import type { MutableRefObject } from 'react'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { layoutSave, titlebarMenu } from '../api'
import { toSnapshot } from '../persist'
import type { Workspace } from '../workspace'

export interface UseWindowLifecycleOptions {
  ws: Workspace | null
  wsRef: MutableRefObject<Workspace | null>
  readyRef: MutableRefObject<boolean>
  reportSaveError: (err: unknown) => void
  flushSettings: () => void
}

export function useWindowLifecycle({
  ws,
  wsRef,
  readyRef,
  reportSaveError,
  flushSettings,
}: UseWindowLifecycleOptions) {
  const [maximized, setMaximized] = useState(false)
  const [quitBusy, setQuitBusy] = useState<number | null>(null)

  // Alt+Space system menu
  useEffect(() => {
    const onKey = (ev: KeyboardEvent) => {
      if (!ev.altKey || ev.code !== 'Space') return
      ev.preventDefault()
      const dpr = window.devicePixelRatio || 1
      void titlebarMenu(Math.round((window.screenX + 16) * dpr), Math.round((window.screenY + 44) * dpr))
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  // Window maximized state watcher
  useEffect(() => {
    const win = getCurrentWindow()
    void win.isMaximized().then(setMaximized)
    const unlisten = win.onResized(() => {
      void win.isMaximized().then(setMaximized)
    })
    return () => {
      void unlisten.then((off) => off())
    }
  }, [])

  // Context menu swallow outside selectable text
  useEffect(() => {
    const onContextMenu = (ev: MouseEvent) => {
      const target = ev.target instanceof Element ? ev.target : null
      if (target?.closest('input, textarea, [contenteditable="true"], [data-selectable]')) return
      ev.preventDefault()
    }
    window.addEventListener('contextmenu', onContextMenu)
    return () => window.removeEventListener('contextmenu', onContextMenu)
  }, [])

  // Auto-save layout on change
  useEffect(() => {
    if (!ws || !readyRef.current) return
    const timer = window.setTimeout(() => {
      void layoutSave(toSnapshot(ws)).catch(reportSaveError)
    }, 500)
    return () => window.clearTimeout(timer)
  }, [ws, readyRef, reportSaveError])

  // Flush layout on quit/restart
  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    void listen('flowdir:flush-layout', () => {
      const current = wsRef.current
      if (current) void layoutSave(toSnapshot(current)).catch(reportSaveError)
      flushSettings()
    }).then((fn) => {
      if (disposed) fn()
      else unlisten = fn
    })
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [flushSettings, reportSaveError, wsRef])

  // Confirm quit event listener
  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    void listen<number>('flowdir:confirm-quit', (event) => setQuitBusy(event.payload)).then((fn) => {
      if (disposed) fn()
      else unlisten = fn
    })
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  return {
    maximized,
    setMaximized,
    quitBusy,
    setQuitBusy,
  }
}
