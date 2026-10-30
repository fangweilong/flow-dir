import React, { forwardRef, useEffect, useImperativeHandle, useRef, useState } from 'react'
import type { Drive } from '../types'
import type { TabState } from '../workspace'
import type { ShortcutId } from '../shortcuts'
import type { Translate } from '../i18n'
import type { MenuPoint } from '../PaneView'
import { formatBinding } from '../shortcuts'
import { pathKey, resolveAddress } from '../paths'
import { executeAddressCommand, expandPath, openTerminal, pathIsDir } from '../api'

export const driveRoot = (path: string | undefined): string => {
  if (!path) return 'C:\\'
  const match = /^[a-zA-Z]:\\/.exec(path)
  return match ? match[0] : path.startsWith('\\\\') ? path.split('\\').slice(0, 4).join('\\') : path
}
export interface AddressBarHandle {
  startAddressEdit: () => void
}

export interface AddressBarProps {
  tab: TabState | undefined
  keys: Record<ShortcutId, string>
  t: Translate
  location: Drive | undefined
  root: boolean
  parts: { label: string; path: string }[]
  upTarget: string
  navButtonStyle: React.CSSProperties
  navFieldStyle: React.CSSProperties
  onHistoryMove: (tabId: string, delta: number) => void
  onNavigate: (tabId: string, path: string) => void
  onDriveMenu: (point: MenuPoint) => void
  onCopyPath: (path: string) => void
  onPathInvalid: (path: string) => void
}

export const AddressBar = forwardRef<AddressBarHandle, AddressBarProps>(function AddressBar(
  {
    tab,
    keys,
    t,
    location,
    root,
    parts,
    upTarget,
    navButtonStyle,
    navFieldStyle,
    onHistoryMove,
    onNavigate,
    onDriveMenu,
    onCopyPath,
    onPathInvalid,
  },
  ref,
) {
  const [addressDraft, setAddressDraft] = useState<string | null>(null)
  const [addressBad, setAddressBad] = useState(false)
  const addressRef = useRef<HTMLInputElement>(null)
  const addressSeq = useRef(0)

  const isEditing = addressDraft !== null
  const prevEditing = useRef(false)
  useEffect(() => {
    if (isEditing && !prevEditing.current) {
      addressRef.current?.focus()
      addressRef.current?.select()
    }
    prevEditing.current = isEditing
  }, [isEditing])

  const startAddressEdit = () => {
    if (!tab) return
    addressSeq.current += 1
    setAddressBad(false)
    setAddressDraft(tab.path || '')
  }

  const closeAddressEdit = () => {
    addressSeq.current += 1
    setAddressDraft(null)
    setAddressBad(false)
  }
  useImperativeHandle(ref, () => ({
    startAddressEdit,
  }))


  const commitAddress = () => {
    if (addressDraft === null || !tab) return
    const raw = addressDraft.trim()
    if (!raw) {
      closeAddressEdit()
      return
    }
    if (raw.toLowerCase() === 'this pc' || raw === t('sidebar.thisPc')) {
      closeAddressEdit()
      if (tab.path) onNavigate(tab.id, '')
      return
    }
    const seq = ++addressSeq.current
    void expandPath(raw)
      .then(async (expanded) => {
        if (seq !== addressSeq.current) return
        const next = resolveAddress(tab.path, expanded)
        if (pathKey(next) === pathKey(tab.path)) {
          closeAddressEdit()
          return
        }
        const isDir = await pathIsDir(next).catch(() => false)
        if (seq !== addressSeq.current) return
        if (isDir) {
          closeAddressEdit()
          onNavigate(tab.id, next)
          return
        }
        const executed = await executeAddressCommand(expanded, tab.path || undefined)
          .then(() => true)
          .catch(() => false)
        if (seq !== addressSeq.current) return
        if (executed) {
          closeAddressEdit()
          return
        }
        setAddressBad(true)
        onPathInvalid(raw)
      })
      .catch(() => {
        if (seq !== addressSeq.current) return
        setAddressBad(true)
        onPathInvalid(raw)
      })
  }

  return (
    <div className="flex min-w-0 flex-1 items-center">
      <button
        type="button"
        title={`${t('nav.back')} (${formatBinding(keys.back)})`}
        aria-label={t('nav.back')}
        onClick={() => tab && onHistoryMove(tab.id, -1)}
        disabled={!tab || tab.histIdx <= 0}
        style={navButtonStyle}
        className="grid place-items-center rounded text-muted hover:bg-surface-3 disabled:opacity-30"
      >
        <svg viewBox="0 0 16 16" className="h-[1.17em] w-[1.17em]" fill="none" stroke="currentColor" strokeWidth="1.6">
          <path d="M10 3 5.5 8 10 13" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </button>
      <button
        type="button"
        title={`${t('nav.forward')} (${formatBinding(keys.forward)})`}
        aria-label={t('nav.forward')}
        onClick={() => tab && onHistoryMove(tab.id, 1)}
        disabled={!tab || tab.histIdx >= tab.history.length - 1}
        style={navButtonStyle}
        className="grid place-items-center rounded text-muted hover:bg-surface-3 disabled:opacity-30"
      >
        <svg viewBox="0 0 16 16" className="h-[1.17em] w-[1.17em]" fill="none" stroke="currentColor" strokeWidth="1.6">
          <path d="M6 3l4.5 5L6 13" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </button>
      <button
        type="button"
        title={`${t('nav.up')} (${formatBinding(keys.up)})`}
        aria-label={t('nav.up')}
        onClick={() => tab && onNavigate(tab.id, upTarget)}
        disabled={!tab || upTarget === tab.path}
        style={navButtonStyle}
        className="grid place-items-center rounded text-muted hover:bg-surface-3 disabled:opacity-30"
      >
        <svg viewBox="0 0 16 16" className="h-[1.17em] w-[1.17em]" fill="none" stroke="currentColor" strokeWidth="1.6">
          <path d="M8 12.5V4M8 4 4.2 7.8M8 4l3.8 3.8" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </button>

      <button
        type="button"
        title={t('nav.drives')}
        onClick={(ev) => {
          const rect = ev.currentTarget.getBoundingClientRect()
          onDriveMenu({
            clientX: rect.left,
            clientY: rect.bottom + 2,
            screenX: window.screenX + rect.left,
            screenY: window.screenY + rect.bottom + 2,
            shiftKey: false,
          })
        }}
        style={navFieldStyle}
        className="ml-1 flex shrink-0 items-center gap-0.5 rounded border border-line bg-surface-2 px-1.5 leading-none text-fg hover:bg-surface-3"
      >
        <span className="font-medium">
          {location ? location.name : root ? t('sidebar.thisPc') : driveRoot(tab?.path)}
        </span>
        <svg viewBox="0 0 16 16" className="h-[1em] w-[1em] text-muted-2" fill="none" stroke="currentColor" strokeWidth="1.6">
          <path d="m4 6.5 4 4 4-4" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </button>

      {addressDraft !== null ? (
        <input
          ref={addressRef}
          data-address-input
          value={addressDraft}
          spellCheck={false}
          onChange={(ev) => {
            setAddressDraft(ev.target.value)
            setAddressBad(false)
          }}
          onBlur={closeAddressEdit}
          onKeyDown={(ev) => {
            if (ev.key === 'Enter') {
              ev.preventDefault()
              commitAddress()
            } else if (ev.key === 'Escape') {
              ev.preventDefault()
              closeAddressEdit()
            }
          }}
          placeholder={root ? t('sidebar.thisPc') : undefined}
          title={addressBad ? t('address.invalidPath') : undefined}
          style={navFieldStyle}
          className={`mx-0.5 min-w-0 flex-1 rounded border bg-surface px-1.5 outline-none ${
            addressBad ? 'border-red-400 text-red-500' : 'border-blue-400'
          }`}
        />
      ) : (
        <div
          style={navFieldStyle}
          className="mx-0.5 flex min-w-0 flex-1 items-center rounded border border-line bg-surface-2 leading-none"
          onClick={(ev) => {
            if (!tab || (ev.target as HTMLElement).closest('button')) return
            startAddressEdit()
          }}
        >
          <div className="no-bar flex h-full min-w-0 flex-1 items-center overflow-x-auto overflow-y-hidden px-1">
            {root && (
              <span className="shrink-0 px-1 py-[0.25em] font-medium leading-none text-fg-strong">
                {t('sidebar.thisPc')}
              </span>
            )}
            {parts.map((part, i) => (
              <div key={part.path} className="flex shrink-0 items-center">
                {i > 0 && <span className="px-0.5 text-muted-2">›</span>}
                <button
                  type="button"
                  data-crumb={part.path}
                  data-drop-dir={part.path}
                  onClick={() => tab && pathKey(part.path) !== pathKey(tab.path) && onNavigate(tab.id, part.path)}
                  title={part.path}
                  className={`max-w-[10rem] truncate rounded px-1 py-[0.25em] leading-none hover:bg-surface-3 ${
                    i === parts.length - 1 ? 'font-medium text-fg-strong' : 'text-muted'
                  }`}
                >
                  {part.label}
                </button>
              </div>
            ))}
          </div>
          {tab?.path && (
            <>
              <button
                type="button"
                data-copy-path
                onClick={() => onCopyPath(tab.path)}
                title={t('address.copyPath')}
                aria-label={t('address.copyPath')}
                className="grid h-full w-[1.67em] shrink-0 place-items-center border-l border-line text-muted-2 hover:bg-surface-3 hover:text-fg"
              >
                <svg viewBox="0 0 16 16" className="h-[1em] w-[1em]" fill="none" stroke="currentColor" strokeWidth="1.4">
                  <rect x="3.5" y="3.5" width="7" height="9" rx="1.2" />
                  <path d="M6 3.5v-.7a1 1 0 0 1 1-1h4.3a1 1 0 0 1 1 1v7.4a1 1 0 0 1-1 1H9.5" />
                </svg>
              </button>
              <button
                type="button"
                data-open-terminal
                onClick={() => tab?.path && void openTerminal(tab.path).catch(() => undefined)}
                title={t('menu.openTerminal')}
                aria-label={t('menu.openTerminal')}
                className="grid h-full w-[1.67em] shrink-0 place-items-center border-l border-line text-muted-2 hover:bg-surface-3 hover:text-fg"
              >
                <svg viewBox="0 0 16 16" className="h-[1em] w-[1em]" fill="none" stroke="currentColor" strokeWidth="1.4">
                  <path d="m4 5 3.5 3L4 11" strokeLinecap="round" strokeLinejoin="round" />
                  <path d="M8.5 11h3.5" strokeLinecap="round" />
                </svg>
              </button>
            </>
          )}
        </div>
      )}
    </div>
  )
})
