import React, { useCallback, useEffect, useRef, useState } from 'react'
import type { Entry, ListData } from '../types'
import type { TabState } from '../workspace'
import type { ShortcutId } from '../shortcuts'
import { matchBinding } from '../shortcuts'
import { matchTypeAhead } from '../typeahead'
import { entryPath, isInsideArchive, joinPath } from '../paths'
import type { MenuPoint } from '../PaneView'
import type { AddressBarHandle } from '../components/AddressBar'

export interface UsePaneKeyboardOptions {
  tab: TabState | undefined
  view: Entry[]
  anchorIdx: number
  visibleSelected: Set<string>
  rowHeight: number
  /** Tiles per row in the grid views; 1 for the details list. */
  columns?: number
  keys: Record<ShortcutId, string>
  /** True while a modal panel owns the window; the list must not act behind it. */
  disabled?: boolean
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
  onNewFolder?: () => void
  onCopyPath?: (path: string) => void
  onCyclePane?: () => void
}

export function usePaneKeyboard({
  tab,
  view,
  anchorIdx,
  visibleSelected,
  rowHeight,
  columns = 1,
  keys,
  disabled,
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
  onNewFolder,
  onCopyPath,
  onCyclePane,
}: UsePaneKeyboardOptions) {
  const [typed, setTyped] = useState('')
  const typeAheadRef = useRef<HTMLInputElement>(null)
  const typeAheadTimer = useRef<number | null>(null)
  // Keyboard cursor. Once Shift+Arrow extends the range the selection anchor stays put,
  // so the row the arrows travel from must be tracked separately from `anchorIdx`.
  const cursorRef = useRef<{ tabId: string; name: string } | null>(null)

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

  const cursorIndex = useCallback(() => {
    const cursor = cursorRef.current
    // A cleared selection (blank click, external reset) or a selection set from elsewhere
    // (search reveal, rename marker) drops the stale cursor in favour of the anchor.
    if (!cursor || !tab || cursor.tabId !== tab.id || !visibleSelected.has(cursor.name)) {
      return anchorIdx
    }
    const index = view.findIndex((e) => e.name === cursor.name)
    return index >= 0 ? index : anchorIdx
  }, [anchorIdx, tab, view, visibleSelected])

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
      cursorRef.current = { tabId: tab.id, name: entry.name }
      update(tab.id, (d) => ({
        ...d,
        anchor: entry.name,
        selected: new Set([entry.name]),
      }))
      virtualizer.scrollToIndex(hit, { align: 'auto' })
    },
    [clearTypeAhead, tab, typed, update, view, virtualizer],
  )

  const selectRow = useCallback(
    (index: number, ev: React.MouseEvent) => {
      const entry = view[index]
      if (!entry || !tab) return
      // A click places the keyboard cursor on the clicked row, while Shift+click keeps
      // the anchor so the range can be re-extended from the same origin.
      cursorRef.current = { tabId: tab.id, name: entry.name }
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
      if (disabled || !tab) return
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

      if (matchBinding(ev, keys.addressBar) || (ev.altKey && (ev.key === 'd' || ev.key === 'D')) || ev.key === 'F4') {
        ev.preventDefault()
        addressBarRef.current?.startAddressEdit()
        return
      }

      if (!tab.path) return

      // A shortcut bound to a bare letter must win over type-ahead, or rebinding
      // rename/refresh to `r` would only jump the selection.
      const boundKey = Object.values(keys).some((binding) => matchBinding(ev, binding))
      if (!boundKey && !ev.ctrlKey && !ev.altKey && !ev.metaKey && ev.key.length === 1) {
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
        cursorRef.current = view[0] ? { tabId: tab.id, name: view[0].name } : null
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
        const name = view[target].name
        if (!extend) {
          cursorRef.current = { tabId: tab.id, name }
          update(tab.id, (d) => ({ ...d, anchor: name, selected: new Set([name]) }))
        } else {
          // Extend from the original anchor: the cursor moves, the anchor does not, so
          // repeated Shift+Arrow keeps growing the range instead of shifting it. With no
          // anchor at all the first extend seeds it once, otherwise it never grows.
          const anchored = anchorIdx >= 0
          const base = anchored ? anchorIdx : target
          const [a, b] = [Math.min(base, target), Math.max(base, target)]
          const selection = new Set(view.slice(a, b + 1).map((e) => e.name))
          cursorRef.current = { tabId: tab.id, name }
          update(tab.id, (d) =>
            anchored ? { ...d, selected: selection } : { ...d, anchor: name, selected: selection },
          )
        }
        virtualizer.scrollToIndex(Math.floor(target / columns), { align: 'auto' })
      }

      if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp' || ev.key === 'ArrowLeft' || ev.key === 'ArrowRight') {
        ev.preventDefault()
        if (!view.length) return
        const step = {
          ArrowDown: columns,
          ArrowUp: -columns,
          ArrowLeft: -1,
          ArrowRight: 1,
        }[ev.key]
        const at = cursorIndex()
        const current = at < 0 ? (step > 0 ? -1 : 0) : at
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
          (Math.floor((scrollRef.current?.clientHeight ?? rowHeight * 10) / rowHeight) - 1) * columns,
        )
        const at = cursorIndex()
        const current = at < 0 ? 0 : at
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
        // After Shift+Arrow the anchor stays at the range start; the row the user
        // travelled to is the keyboard cursor.
        const entry = view[cursorIndex()]
        if (entry) activate(entry)
        return
      }

      if (ev.key === 'Escape') {
        ev.preventDefault()
        cursorRef.current = null
        update(tab.id, (d) => ({ ...d, selected: new Set(), anchor: '' }))
        return
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
        const entry = view[cursorIndex()]
        const name = entry?.name
        const row = name ? sectionRef.current?.querySelector<HTMLElement>(`[data-row="${CSS.escape(name)}"]`) : null
        if (entry && row) {
          ev.preventDefault()
          const rect = row.getBoundingClientRect()
          onRowContextMenu(entry, {
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
            cursorRef.current = null
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
      if (matchBinding(ev, keys.newFolder)) {
        if (isInsideArchive(tab.path)) return
        ev.preventDefault()
        onNewFolder?.()
        return
      }

      if (matchBinding(ev, keys.copyPath)) {
        ev.preventDefault()
        if (visibleSelected.size > 0) {
          const byName = new Map(view.map((item) => [item.name, item]))
          const paths = Array.from(visibleSelected).map((name) => {
            const item = byName.get(name)
            return item ? entryPath(tab.path, item) : joinPath(tab.path, name)
          })
          onCopyPath?.(paths.join('\n'))
        } else if (tab.path) {
          onCopyPath?.(tab.path)
        }
        return
      }

      if (matchBinding(ev, keys.invertSelection)) {
        ev.preventDefault()
        const current = visibleSelected
        const inverted = view
          .map((e) => e.name)
          .filter((name) => !current.has(name))
        cursorRef.current = inverted[0] ? { tabId: tab.id, name: inverted[0] } : null
        update(tab.id, (d) => ({
          ...d,
          selected: new Set(inverted),
          anchor: inverted[0] ?? '',
        }))
        return
      }

      if (matchBinding(ev, keys.cyclePane) || (ev.key === 'Tab' && !ev.ctrlKey && !ev.altKey && !ev.metaKey)) {
        ev.preventDefault()
        onCyclePane?.()
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
      cursorIndex,
      disabled,
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
      onNewFolder,
      onCopyPath,
      onCyclePane,
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
