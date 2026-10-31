import { useCallback, useEffect, useRef, useState } from 'react'
import type { RefObject } from 'react'
import type { GitStatus } from '../types'
import { EMPTY_GIT } from '../types'
import type { Settings } from '../persist'
import type { Workspace } from '../workspace'
import { gitStatus, treeDirs } from '../api'
import { isFsPath, pathKey } from '../paths'

export const GIT_CACHE_LIMIT = 64

export interface UseTreeAndGitProps {
  settingsRef: RefObject<Settings>
  sidebarOpen: boolean
  showHidden: boolean
  ws: Workspace | null
}

export function useTreeAndGit({
  settingsRef,
  sidebarOpen,
  showHidden,
  ws,
}: UseTreeAndGitProps) {
  const [treeOpen, setTreeOpen] = useState<Record<string, string>>({})
  const [treeNodes, setTreeNodes] = useState<Record<string, string[]>>({})
  const [treeTruncated, setTreeTruncated] = useState<Record<string, boolean>>({})
  const [treeErrors, setTreeErrors] = useState<Record<string, boolean>>({})
  const [gitByDir, setGitByDir] = useState<Record<string, GitStatus>>({})
  const [gitEpoch, setGitEpoch] = useState(0)

  const treeOpenRef = useRef<Record<string, string>>({})
  const treeNodesRef = useRef<Record<string, string[]>>({})
  const treeErrorsRef = useRef<Record<string, boolean>>({})
  const treePending = useRef(new Set<string>())
  const treeGeneration = useRef(new Map<string, number>())
  const treeCounter = useRef(0)

  const gitByDirRef = useRef<Record<string, GitStatus>>({})
  const gitPending = useRef(new Set<string>())
  const gitSeq = useRef(new Map<string, number>())

  useEffect(() => {
    gitByDirRef.current = gitByDir
  }, [gitByDir])

  const loadTree = useCallback((path: string, force = false) => {
    const key = pathKey(path)
    if (!isFsPath(path) || treePending.current.has(key)) return
    if (!force && (treeNodesRef.current[key] || treeErrorsRef.current[key])) return
    treePending.current.add(key)
    const generation = ++treeCounter.current
    treeGeneration.current.set(key, generation)
    const reply = (names: string[], truncated: boolean) => {
      if (treeGeneration.current.get(key) !== generation) return
      treeNodesRef.current = { ...treeNodesRef.current, [key]: names }
      setTreeNodes(treeNodesRef.current)
      setTreeTruncated((prev) =>
        prev[key] === truncated ? prev : { ...prev, [key]: truncated },
      )
      if (treeErrorsRef.current[key]) {
        const rest = { ...treeErrorsRef.current }
        delete rest[key]
        treeErrorsRef.current = rest
        setTreeErrors(rest)
      }
    }
    treeDirs(path, settingsRef.current?.showHidden ?? false)
      .then((listing) => reply(listing.names, listing.truncated))
      .catch(() => {
        if (treeGeneration.current.get(key) !== generation) return
        treeErrorsRef.current = { ...treeErrorsRef.current, [key]: true }
        setTreeErrors(treeErrorsRef.current)
      })
      .finally(() => {
        // `invalidateTree` bumps the generation to drop the in-flight reply, but the
        // key must not stay in `treePending` or every later load (even forced) is
        // refused until a showHidden reset clears the set.
        treePending.current.delete(key)
      })
  }, [settingsRef])

  const invalidateTree = useCallback((path: string) => {
    const key = pathKey(path)
    treeGeneration.current.set(key, ++treeCounter.current)
    if (!(key in treeNodesRef.current) && !treeErrorsRef.current[key]) return
    if (key in treeNodesRef.current) {
      const next = { ...treeNodesRef.current }
      delete next[key]
      treeNodesRef.current = next
      setTreeNodes(next)
    }
    if (treeErrorsRef.current[key]) {
      const rest = { ...treeErrorsRef.current }
      delete rest[key]
      treeErrorsRef.current = rest
      setTreeErrors(rest)
    }
    setTreeTruncated((prev) => {
      if (!(key in prev)) return prev
      const rest = { ...prev }
      delete rest[key]
      return rest
    })
  }, [])

  const toggleTree = useCallback(
    (path: string) => {
      const key = pathKey(path)
      const opening = !(key in treeOpenRef.current)
      if (opening) treeOpenRef.current = { ...treeOpenRef.current, [key]: path }
      setTreeOpen((prev) => {
        if (!(key in prev)) return { ...prev, [key]: path }
        const next = { ...prev }
        delete next[key]
        return next
      })
      if (opening) {
        if (treeErrorsRef.current[key]) {
          const rest = { ...treeErrorsRef.current }
          delete rest[key]
          treeErrorsRef.current = rest
          setTreeErrors(rest)
        }
        loadTree(path, true)
      }
    },
    [loadTree],
  )

  useEffect(() => {
    treeOpenRef.current = treeOpen
    if (!sidebarOpen) return
    for (const [key, path] of Object.entries(treeOpen)) {
      if (!treeNodes[key] && !treeErrors[key]) loadTree(path)
    }
  }, [sidebarOpen, treeOpen, treeNodes, treeErrors, loadTree])

  useEffect(() => {
    for (const key of new Set([...Object.keys(treeNodesRef.current), ...treePending.current])) {
      treeGeneration.current.set(key, ++treeCounter.current)
    }
    treeNodesRef.current = {}
    treePending.current.clear()
    treeErrorsRef.current = {}
    setTreeNodes({})
    setTreeTruncated({})
    setTreeErrors({})
  }, [showHidden])

  const loadGit = useCallback((dir: string) => {
    const key = pathKey(dir)
    if (!dir || gitPending.current.has(key)) return
    const seq = (gitSeq.current.get(key) ?? 0) + 1
    gitSeq.current.set(key, seq)
    gitPending.current.add(key)
    const store = (status: GitStatus) =>
      setGitByDir((prev) => {
        if (gitSeq.current.get(key) !== seq) return prev
        const next = { ...prev, [key]: status }
        const keys = Object.keys(next)
        for (const stale of keys.slice(0, Math.max(0, keys.length - GIT_CACHE_LIMIT))) delete next[stale]
        return next
      })
    gitStatus(dir)
      .then(store)
      .catch(() => store(EMPTY_GIT))
      .finally(() => {
        if (gitSeq.current.get(key) === seq) gitPending.current.delete(key)
      })
  }, [])

  const invalidateGit = useCallback((dir: string) => {
    const key = pathKey(dir)
    gitSeq.current.set(key, (gitSeq.current.get(key) ?? 0) + 1)
    gitPending.current.delete(key)
    setGitEpoch((epoch) => epoch + 1)
    setGitByDir((prev) => {
      if (!(key in prev)) return prev
      const next = { ...prev }
      delete next[key]
      return next
    })
  }, [])

  useEffect(() => {
    if (!ws) return
    const missing: string[] = []
    for (const pane of Object.values(ws.panes)) {
      const tab = ws.tabs[pane.activeTabId]
      if (tab?.path && !(pathKey(tab.path) in gitByDir)) missing.push(tab.path)
    }
    if (!missing.length) return
    const timer = window.setTimeout(() => {
      for (const dir of missing) loadGit(dir)
    }, 150)
    return () => window.clearTimeout(timer)
  }, [ws, gitByDir, gitEpoch, loadGit])

  return {
    treeOpen,
    setTreeOpen,
    treeNodes,
    treeTruncated,
    treeErrors,
    gitByDir,
    gitByDirRef,
    loadTree,
    invalidateTree,
    toggleTree,
    loadGit,
    invalidateGit,
  }
}
