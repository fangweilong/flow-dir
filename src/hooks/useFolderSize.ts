import { useCallback, useRef, useState } from 'react'
import { folderSizeStart, toolCancel } from '../api'
import type { FolderSizeProgress } from '../api'
import type { ApiError } from '../types'
import { isFsPath } from '../paths'

export type FolderSizeState = FolderSizeProgress & {
  tabId: string
  /** Folder the result belongs to; a tab that navigated away must not show it. */
  path: string
}

/**
 * One recursive-size job per tab at a time. The result is kept until the tab starts
 * another run or navigates elsewhere; `toolCancel` stops a walk that is still reading.
 */
export function useFolderSize() {
  const [byTab, setByTab] = useState<Record<string, FolderSizeState>>({})
  const jobs = useRef<Record<string, string>>({})

  const start = useCallback((tabId: string, path: string) => {
    // Shell-namespace folders have no filesystem tree to walk.
    if (!path || !isFsPath(path)) return
    const previous = jobs.current[tabId]
    if (previous) void toolCancel(previous)
    const jobId = `size-${tabId}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
    jobs.current[tabId] = jobId
    setByTab((prev) => ({
      ...prev,
      [tabId]: {
        tabId,
        path,
        files: 0,
        dirs: 0,
        bytes: 0,
        done: false,
        aborted: false,
        error: null,
      },
    }))
    void folderSizeStart(jobId, path, (progress) => {
      if (jobs.current[tabId] !== jobId) return
      setByTab((prev) => {
        const current = prev[tabId]
        if (!current || current.path !== path) return prev
        return { ...prev, [tabId]: { ...current, ...progress, tabId, path } }
      })
      if (progress.done) delete jobs.current[tabId]
    }).catch((err: unknown) => {
      if (jobs.current[tabId] !== jobId) return
      delete jobs.current[tabId]
      setByTab((prev) => {
        const current = prev[tabId]
        if (!current || current.path !== path) return prev
        return {
          ...prev,
          [tabId]: { ...current, done: true, error: err as ApiError },
        }
      })
    })
  }, [])

  const cancel = useCallback((tabId: string) => {
    const jobId = jobs.current[tabId]
    if (!jobId) return
    void toolCancel(jobId)
  }, [])

  const clear = useCallback((tabId: string) => {
    delete jobs.current[tabId]
    setByTab((prev) => {
      if (!(tabId in prev)) return prev
      const next = { ...prev }
      delete next[tabId]
      return next
    })
  }, [])

  return { byTab, start, cancel, clear }
}