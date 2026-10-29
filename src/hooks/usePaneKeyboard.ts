import React, { useCallback, useEffect, useRef, useState } from 'react'
import type { Entry, ListData } from '../types'
import type { TabState } from '../workspace'
import type { ShortcutId } from '../shortcuts'
import { matchBinding } from '../shortcuts'
import { matchTypeAhead } from '../typeahead'
import { isInsideArchive } from '../paths'
import type { MenuPoint } from '../PaneView'
import type { AddressBarHandle } from '../components/AddressBar'

export interface UsePaneKeyboardOptions {
  tab: TabState | undefined
  view: Entry[]
  anchorIdx: number
  visibleSelected: Set<string>
  rowHeight: number
  keys: Record<ShortcutId, string>
  scrollRef: React.RefObject<HTMLDivElement | null>
  sectionRef: React.RefObject<HTMLElement | null>
  addressBarRef: React.RefObject<AddressBarHandle | null>
  virtualizer: {
    scrollToIndex: (index: number, options?: { align: 'auto' | 'start' | 'center' | 'end' }) => void
  }
  upTarget: string
  update: (tabId: string, fn: (d: ListData) => ListData) => void
  activate: (entry: Entry) => void
  onProperties: (names: string[]) => void
  onHistoryMove: (tabId: string, delta: number) => void
  onNavigate: (tabId: string, path: string) => void
  onClipboard: (cut: boolean) => void
  onRename: (name: string) => void
  onRenameMany: (names: string[]) => void
  onDelete: (names: string[], permanent: boolean) => void
  onRefresh: () => void
  onPaste: () => void
  onRowContextMenu: (entry: Entry, point: MenuPoint) => void
  onBlankContextMenu: (point: MenuPoint) => void
}

export function usePaneKeyboard({
  tab,
  view,
  anchorIdx,
  visibleSelected,
  rowHeight,
  keys,
  scrollRef,
  sectionRef,
  addressBarRef,
  virtualizer,
  upTarget,
  update,
  activate,
  onProperties,
  onHistoryMove,
  onNavigate,
  onClipboard,
  onRename,
  onRenameMany,
  onDelete,
  onRefresh,
  onPaste,
  onRowContextMenu,
  onBlankContextMenu,
}: UsePaneKeyboardOptions) {
  const [typed, setTyped] = useState('')
  const typeAheadRef = useRef<HTMLInputElement>(null)
  const typeAheadTimer = useRef<number | null>(null)

  useEffect(
    () => () => {
      if (typeAheadTimer.current) window.clearTimeout(typeAheadTimer.current)
    },
    [],
  )

  const clearTypeAhead = useCallback(() => {
    if (typeAheadTimer.current) window.clearTimeout(typeAheadTimer.current)
    typeAheadTimer.current = null
    setTyped('')
  }, [])

  const typeAhead = useCallback(
    (char: string) => {
      if (!tab) return
      const next = typed + char
      setTyped(next)
      if (typeAheadTimer.current) window.clearTimeout(typeAheadTimer.current)
      typeAheadTimer.current = window.setTimeout(clearTypeAhead, 800)
      const hit = matchTypeAhead(
        view.map((e) => e.name),
        next,
      )
      const entry = view[hit]
      if (!entry) return
      update(tab.id, (d) => ({
        ...d,
        anchor: entry.name,
        selected: new Set([entry.name]),
      }))
      virtualizer.scrollToIndex(hit, { align: 'auto' })
    },
    [anchorIdx, clearTypeAhead, tab, typed, update, view, virtualizer],
  )

  const selectRow = useCallback(
    (index: number, ev: React.MouseEvent) => {
      const entry = view[index]
      if (!entry || !tab) return
      update(tab.id, (d) => {
        if (ev.shiftKey && anchorIdx >= 0) {
          const [a, b] = [Math.min(anchorIdx, index), Math.max(anchorIdx, index)]
          return { ...d, selected: new Set(view.slice(a, b + 1).map((e) => e.name)) }
        }
        if (ev.ctrlKey || ev.metaKey) {
          const next = new Set(d.selected)
          if (next.has(entry.name)) next.delete(entry.name)
          else next.add(entry.name)
          return { ...d, selected: next, anchor: entry.name }
        }
        return { ...d, selected: new Set([entry.name]), anchor: entry.name }
      })
    },
    [anchorIdx, tab, update, view],
  )

  const onKeyDown = useCallback(
    (ev: React.KeyboardEvent) => {
      if (!tab) return
      if (ev.nativeEvent.isComposing) return

      if (typed) {
        if (ev.key === 'Backspace') {
          ev.preventDefault()
          setTyped(typed.slice(0, -1))
          return
        }
        if (ev.key === 'Escape') {
          ev.preventDefault()
          clearTypeAhead()
          return
        }
        if (ev.key === 'Enter') {
          ev.preventDefault()
          const entry = view[anchorIdx]
          clearTypeAhead()
          if (entry) activate(entry)
          return
        }
        clearTypeAhead()
      }

      if (matchBinding(ev, keys.properties)) {
        ev.preventDefault()
        const names = visibleSelected.size
          ? Array.from(visibleSelected)
          : view[anchorIdx]
            ? [view[anchorIdx].name]
            : []
        if (names.length) onProperties(names)
        return
      }

      if (matchBinding(ev, keys.back)) {
        ev.preventDefault()
        onHistoryMove(tab.id, -1)
        return
      }

      if (matchBinding(ev, keys.forward)) {
        ev.preventDefault()
        onHistoryMove(tab.id, 1)
        return
      }

      if (matchBinding(ev, keys.up) || (ev.altKey && ev.key === 'ArrowUp')) {
        ev.preventDefault()
        onNavigate(tab.id, upTarget)
        return
      }

      if (matchBinding(ev, keys.addressBar)) {
        ev.preventDefault()
        addressBarRef.current?.startAddressEdit()
        return
      }

      if (!tab.path) return

      if (!ev.ctrlKey && !ev.altKey && !ev.metaKey && ev.key.length === 1) {
        ev.preventDefault()
        typeAhead(ev.key)
        return
      }

      if (matchBinding(ev, keys.copy) || matchBinding(ev, keys.cut)) {
        if (matchBinding(ev, keys.cut) && isInsideArchive(tab.path)) return
        ev.preventDefault()
        onClipboard(matchBinding(ev, keys.cut))
        return
      }

      if (matchBinding(ev, keys.selectAll)) {
        ev.preventDefault()
        update(tab.id, (d) => ({
          ...d,
          selected: new Set(view.map((e) => e.name)),
          anchor: view[0]?.name ?? '',
        }))
        return
      }

      const moveTo = (next: number, extend: boolean) => {
        if (!view.length) return
        const target = Math.max(0, Math.min(view.length - 1, next))
        const base = anchorIdx < 0 ? target : anchorIdx
        update(tab.id, (d) => {
          if (!extend) {
            return { ...d, anchor: view[target].name, selected: new Set([view[target].name]) }
          }
          const [a, b] = [Math.min(base, target), Math.max(base, target)]
          return {
            ...d,
            anchor: view[target].name,
            selected: new Set(view.slice(a, b + 1).map((e) => e.name)),
          }
        })
        virtualizer.scrollToIndex(target, { align: 'auto' })
      }

      if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') {
        ev.preventDefault()
        if (!view.length) return
        const step = ev.key === 'ArrowDown' ? 1 : -1
        const current = anchorIdx < 0 ? (step > 0 ? -1 : 0) : anchorIdx
        moveTo(current + step, ev.shiftKey)
        return
      }

      if (
        !ev.ctrlKey &&
        !ev.altKey &&
        !ev.metaKey &&
        (ev.key === 'Home' || ev.key === 'End' || ev.key === 'PageUp' || ev.key === 'PageDown')
      ) {
        ev.preventDefault()
        const page = Math.max(
          1,
          Math.floor((scrollRef.current?.clientHeight ?? rowHeight * 10) / rowHeight) - 1,
        )
        const current = anchorIdx < 0 ? 0 : anchorIdx
        const next =
          ev.key === 'Home'
            ? 0
            : ev.key === 'End'
              ? view.length - 1
              : current + (ev.key === 'PageDown' ? page : -page)
        moveTo(next, ev.shiftKey)
        return
      }

      if (matchBinding(ev, keys.open)) {
        const entry = view[anchorIdx]
        if (entry) activate(entry)
        return
      }

      if (ev.key === 'Escape') {
        update(tab.id, (d) => ({ ...d, selected: new Set(), anchor: '' }))
      }

      if (matchBinding(ev, keys.rename)) {
        if (isInsideArchive(tab.path)) return
        ev.preventDefault()
        if (visibleSelected.size > 1) {
          onRenameMany(Array.from(visibleSelected))
          return
        }
        const target =
          visibleSelected.size === 1 ? Array.from(visibleSelected)[0] : view[anchorIdx]?.name
        if (target) onRename(target)
        return
      }

      if (matchBinding(ev, keys.delete, { ignoreShift: true })) {
        if (isInsideArchive(tab.path)) return
        if (!visibleSelected.size) return
        ev.preventDefault()
        onDelete(Array.from(visibleSelected), ev.shiftKey)
        return
      }

      if (matchBinding(ev, keys.refresh)) {
        ev.preventDefault()
        onRefresh()
        return
      }

      if (matchBinding(ev, keys.paste)) {
        if (isInsideArchive(tab.path)) return
        ev.preventDefault()
        onPaste()
        return
      }

      if (matchBinding(ev, keys.contextMenu)) {
        const name = view[anchorIdx]?.name
        const row = name ? sectionRef.current?.querySelector<HTMLElement>(`[data-row="${CSS.escape(name)}"]`) : null
        if (name && row) {
          ev.preventDefault()
          const rect = row.getBoundingClientRect()
          onRowContextMenu(view[anchorIdx], {
            clientX: rect.left + 40,
            clientY: rect.top + 8,
            screenX: window.screenX + rect.left + 40,
            screenY: window.screenY + rect.top + 8,
            shiftKey: false,
          })
        } else if (tab) {
          const rect = scrollRef.current?.getBoundingClientRect()
          if (rect) {
            ev.preventDefault()
            update(tab.id, (d) => ({ ...d, selected: new Set(), anchor: '' }))
            onBlankContextMenu({
              clientX: rect.left + 40,
              clientY: rect.top + 8,
              screenX: window.screenX + rect.left + 40,
              screenY: window.screenY + rect.top + 8,
              shiftKey: false,
            })
          }
        }
        return
      }
    },
    [
      tab,
      typed,
      keys,
      visibleSelected,
      view,
      anchorIdx,
      onProperties,
      onHistoryMove,
      onNavigate,
      upTarget,
      addressBarRef,
      typeAhead,
      onClipboard,
      update,
      virtualizer,
      scrollRef,
      rowHeight,
      activate,
      clearTypeAhead,
      onRenameMany,
      onRename,
      onDelete,
      onRefresh,
      onPaste,
      sectionRef,
      onRowContextMenu,
      onBlankContextMenu,
    ],
  )

  return {
    typed,
    setTyped,
    typeAheadRef,
    typeAhead,
    clearTypeAhead,
    selectRow,
    onKeyDown,
  }
}
