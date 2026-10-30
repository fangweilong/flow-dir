import { useEffect } from 'react'
import type { ShortcutId } from '../shortcuts'
import { isTextInput, matchBinding } from '../shortcuts'
import type { PaneState, TabState } from '../workspace'
import type { ListData } from '../types'
import { isDir, joinPath } from '../paths'

export interface UseGlobalShortcutsOptions {
  keys: Record<ShortcutId, string>
  activePane: PaneState | undefined
  activeTab: TabState | undefined
  activeList: ListData | undefined
  previewName: string
  overlayOpen: boolean
  activateTab: (paneId: string, tabId: string) => void
  newTab: (paneId: string) => void
  closeTab: (paneId: string, tabId: string) => void
  reopenTab: () => void
  setSearchOpen: (open: boolean) => void
  setSettingsOpen: (open: boolean) => void
  setPreviewOpen: React.Dispatch<React.SetStateAction<boolean>>
  toggleSidebar: () => void
  undoLast: () => void
  redoLast: () => void
  openExternalPreview: (target: string) => void
  cyclePane?: () => void
}

export function useGlobalShortcuts({
  keys,
  activePane,
  activeTab,
  activeList,
  previewName,
  overlayOpen,
  activateTab,
  newTab,
  closeTab,
  reopenTab,
  setSearchOpen,
  setSettingsOpen,
  setPreviewOpen,
  toggleSidebar,
  undoLast,
  redoLast,
  openExternalPreview,
  cyclePane,
}: UseGlobalShortcutsOptions) {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (overlayOpen) return
      const bare = !event.ctrlKey && !event.altKey && !event.shiftKey && !event.metaKey
      if (bare && isTextInput(event.target)) return

      if (activePane && event.ctrlKey && !event.altKey && !event.metaKey && event.key === 'Tab') {
        event.preventDefault()
        const ids = activePane.tabIds
        if (ids.length > 1) {
          const index = ids.indexOf(activePane.activeTabId)
          const step = event.shiftKey ? -1 : 1
          const next = ids[(index + step + ids.length) % ids.length]
          if (next) activateTab(activePane.id, next)
        }
        return
      }
      if (
        activePane &&
        event.ctrlKey &&
        !event.altKey &&
        !event.metaKey &&
        !event.shiftKey &&
        /^[1-9]$/.test(event.key)
      ) {
        const ids = activePane.tabIds
        const id = event.key === '9' ? ids[ids.length - 1] : ids[Number(event.key) - 1]
        if (id) {
          event.preventDefault()
          activateTab(activePane.id, id)
        }
        return
      }

      if (matchBinding(event, keys.search)) {
        event.preventDefault()
        setSearchOpen(true)
        return
      }
      if (matchBinding(event, keys.undo)) {
        if (isTextInput(event.target)) return
        event.preventDefault()
        undoLast()
        return
      }
      if (matchBinding(event, keys.redo)) {
        if (isTextInput(event.target)) return
        event.preventDefault()
        redoLast()
        return
      }
      if (matchBinding(event, keys.togglePreview)) {
        event.preventDefault()
        setPreviewOpen((open) => !open)
        return
      }
      if (matchBinding(event, keys.toggleSidebar)) {
        event.preventDefault()
        toggleSidebar()
        return
      }
      if (matchBinding(event, keys.openSettings)) {
        event.preventDefault()
        setSettingsOpen(true)
        return
      }
      if (activePane && matchBinding(event, keys.newTab)) {
        event.preventDefault()
        newTab(activePane.id)
        return
      }
      if (matchBinding(event, keys.reopenTab)) {
        event.preventDefault()
        reopenTab()
        return
      }
      if (activePane && activeTab && matchBinding(event, keys.closeTab)) {
        event.preventDefault()
        closeTab(activePane.id, activeTab.id)
        return
      }
      if (matchBinding(event, keys.externalPreview)) {
        if (!activeTab || !previewName) return
        const entry = activeList?.entries.find((item) => item.name === previewName)
        if (!entry || isDir(entry.attrs)) return
        event.preventDefault()
        openExternalPreview(joinPath(activeTab.path, previewName))
      }
      if (matchBinding(event, keys.cyclePane)) {
        event.preventDefault()
        cyclePane?.()
        return
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [
    keys,
    overlayOpen,
    activePane,
    activeTab,
    activeList,
    previewName,
    activateTab,
    newTab,
    closeTab,
    reopenTab,
    setSearchOpen,
    setSettingsOpen,
    setPreviewOpen,
    toggleSidebar,
    undoLast,
    redoLast,
    openExternalPreview,
    cyclePane,
  ])
}
