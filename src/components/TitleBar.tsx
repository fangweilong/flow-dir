import React from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import iconUrl from '../assets/icon.png'
import type { Translate } from '../i18n'
import type { ShortcutId } from '../shortcuts'
import { formatBinding } from '../shortcuts'

type CaptionButtonProps = {
  label: string
  danger?: boolean
  onClick: () => void
  children: React.ReactNode
}

function CaptionButton({ label, danger, onClick, children }: CaptionButtonProps) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
      className={`grid w-[2.875rem] place-items-center text-muted transition-colors ${
        danger ? 'hover:bg-[#c42b1c] hover:text-white' : 'hover:bg-surface-3 hover:text-fg'
      }`}
    >
      <svg viewBox="0 0 16 16" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="1.2">
        {children}
      </svg>
    </button>
  )
}

function LayoutButton({
  title,
  active,
  onClick,
  kind,
  dir,
}: {
  title: string
  active: boolean
  onClick: () => void
  kind: 1 | 2 | 3 | 4
  dir?: 'row' | 'col'
}) {
  const rects: React.ReactNode[] = []
  if (kind === 1) {
    rects.push(<rect key="a" x="2" y="2" width="12" height="12" rx="1.5" />)
  } else if (kind === 2 && dir === 'row') {
    rects.push(<rect key="a" x="2" y="2" width="5.4" height="12" rx="1.2" />)
    rects.push(<rect key="b" x="8.6" y="2" width="5.4" height="12" rx="1.2" />)
  } else if (kind === 2) {
    rects.push(<rect key="a" x="2" y="2" width="12" height="5.4" rx="1.2" />)
    rects.push(<rect key="b" x="2" y="8.6" width="12" height="5.4" rx="1.2" />)
  } else if (kind === 3) {
    rects.push(<rect key="a" x="2" y="2" width="5.4" height="12" rx="1.2" />)
    rects.push(<rect key="b" x="8.6" y="2" width="5.4" height="5.4" rx="1.2" />)
    rects.push(<rect key="c" x="8.6" y="8.6" width="5.4" height="5.4" rx="1.2" />)
  } else {
    rects.push(<rect key="a" x="2" y="2" width="5.4" height="5.4" rx="1.2" />)
    rects.push(<rect key="b" x="8.6" y="2" width="5.4" height="5.4" rx="1.2" />)
    rects.push(<rect key="c" x="2" y="8.6" width="5.4" height="5.4" rx="1.2" />)
    rects.push(<rect key="d" x="8.6" y="8.6" width="5.4" height="5.4" rx="1.2" />)
  }
  return (
    <button
      type="button"
      title={title}
      onClick={onClick}
      className={`grid h-5 w-6 place-items-center rounded ${
        active ? 'bg-surface text-blue-500 shadow-sm' : 'text-muted-2 hover:text-fg'
      }`}
    >
      <svg viewBox="0 0 16 16" className="h-3 w-3" fill="currentColor">
        {rects}
      </svg>
    </button>
  )
}

export type TitleBarProps = {
  t: Translate
  activePath: string
  layoutCount: number
  layoutDir?: 'row' | 'col'
  applyLayout: (kind: 1 | 2 | 3 | 4, orient: 'h' | 'v') => void
  toggleSidebar: () => void
  sidebarOpen: boolean
  keys: Record<ShortcutId, string>
  historyOpen: boolean
  setHistoryOpen: React.Dispatch<React.SetStateAction<boolean>>
  comparePathsLength: number
  setCompareOpen: (val: boolean) => void
  setSearchOpen: (val: boolean) => void
  previewOpen: boolean
  setPreviewOpen: React.Dispatch<React.SetStateAction<boolean>>
  setSettingsOpen: (val: boolean) => void
  updateAvailable: string | null
  maximized: boolean
  setMaximized: (val: boolean) => void
  onContextMenu: (ev: React.MouseEvent) => void
}

export function TitleBar({
  t,
  activePath,
  layoutCount,
  layoutDir,
  applyLayout,
  toggleSidebar,
  sidebarOpen,
  keys,
  historyOpen,
  setHistoryOpen,
  comparePathsLength,
  setCompareOpen,
  setSearchOpen,
  previewOpen,
  setPreviewOpen,
  setSettingsOpen,
  updateAvailable,
  maximized,
  setMaximized,
  onContextMenu,
}: TitleBarProps) {
  return (
    <header
      data-tauri-drag-region
      onContextMenu={onContextMenu}
      className="flex h-8 shrink-0 items-center gap-2 border-b border-line bg-surface/90 pl-2"
    >
      <div className="flex items-center gap-2" data-tauri-drag-region>
        <img src={iconUrl} alt="" className="h-5 w-5 select-none" draggable={false} />
        <span data-tauri-drag-region className="text-[0.8125rem] font-semibold tracking-tight">
          {t('app.name')}
        </span>
      </div>

      <div className="mx-1.5 h-4 w-px bg-line" />

      <div className="flex items-center gap-0.5 rounded-md bg-surface-3 p-0.5">
        <LayoutButton title={t('layout.single')} active={layoutCount === 1} onClick={() => applyLayout(1, 'h')} kind={1} />
        <LayoutButton
          title={t('layout.twoH')}
          active={layoutCount === 2 && layoutDir === 'row'}
          onClick={() => applyLayout(2, 'h')}
          kind={2}
          dir="row"
        />
        <LayoutButton
          title={t('layout.twoV')}
          active={layoutCount === 2 && layoutDir === 'col'}
          onClick={() => applyLayout(2, 'v')}
          kind={2}
          dir="col"
        />
        <LayoutButton title={t('layout.three')} active={layoutCount === 3} onClick={() => applyLayout(3, 'h')} kind={3} />
        <LayoutButton title={t('layout.four')} active={layoutCount === 4} onClick={() => applyLayout(4, 'h')} kind={4} />
      </div>

      <div data-tauri-drag-region className="ml-2 min-w-0 flex-1 truncate text-[0.6875rem] text-muted-2">
        {activePath}
      </div>
      <div data-tauri-drag-region className="hidden shrink-0 text-[0.6875rem] text-muted-2 lg:block">
        {t('layout.hint', { count: layoutCount })}
      </div>
      <button
        type="button"
        onClick={toggleSidebar}
        title={`${t('sidebar.toggle')} (${formatBinding(keys.toggleSidebar)})`}
        aria-label={t('sidebar.toggle')}
        className={`grid h-6 w-6 shrink-0 place-items-center rounded-md hover:bg-surface-3 ${
          sidebarOpen ? 'text-muted' : 'text-muted-2'
        }`}
      >
        <svg viewBox="0 0 16 16" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="1.4">
          <rect x="2" y="3" width="12" height="10" rx="1.5" />
          <path d="M6 3v10" />
        </svg>
      </button>
      <button
        type="button"
        onClick={() => setHistoryOpen((open) => !open)}
        title={t('history.open')}
        aria-label={t('history.open')}
        data-history-toggle
        className={`grid h-6 w-6 shrink-0 place-items-center rounded-md hover:bg-surface-3 ${
          historyOpen ? 'text-blue-500' : 'text-muted'
        }`}
      >
        <svg viewBox="0 0 16 16" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="1.4">
          <path d="M2.6 7.2A5.6 5.6 0 1 1 3.4 10" strokeLinecap="round" />
          <path d="M2.2 3.4v3.6h3.6" strokeLinecap="round" strokeLinejoin="round" />
          <path d="M8 4.8V8l2.2 1.4" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </button>
      <button
        type="button"
        onClick={() => setCompareOpen(true)}
        disabled={comparePathsLength < 1}
        title={comparePathsLength < 2 ? t('compare.needPane') : t('compare.open')}
        aria-label={t('compare.open')}
        className="grid h-6 w-6 shrink-0 place-items-center rounded-md text-muted hover:bg-surface-3 hover:text-fg disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-muted"
      >
        <svg viewBox="0 0 16 16" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="1.4">
          <rect x="2" y="3.5" width="5" height="9" rx="1" />
          <rect x="9" y="3.5" width="5" height="9" rx="1" />
          <path d="M7 8h2" strokeLinecap="round" />
        </svg>
      </button>
      <button
        type="button"
        onClick={() => setSearchOpen(true)}
        title={`${t('search.open')} (${formatBinding(keys.search)})`}
        aria-label={t('search.open')}
        className="grid h-6 w-6 shrink-0 place-items-center rounded-md text-muted hover:bg-surface-3 hover:text-fg"
      >
        <svg viewBox="0 0 16 16" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="1.5">
          <circle cx="7" cy="7" r="4.2" />
          <path d="m10.2 10.2 3 3" strokeLinecap="round" />
        </svg>
      </button>
      <button
        type="button"
        onClick={() => setPreviewOpen((open) => !open)}
        title={`${t('preview.title')} (${formatBinding(keys.togglePreview)})`}
        aria-label={t('preview.title')}
        className={`grid h-6 w-6 shrink-0 place-items-center rounded-md hover:bg-surface-3 ${
          previewOpen ? 'text-blue-500' : 'text-muted'
        }`}
      >
        <svg viewBox="0 0 16 16" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="1.4">
          <rect x="2" y="2.5" width="12" height="11" rx="1.5" />
          <path d="M10 2.5v11" />
        </svg>
      </button>
      <button
        type="button"
        onClick={() => setSettingsOpen(true)}
        title={t('app.settings')}
        className="relative grid h-6 w-6 shrink-0 place-items-center rounded-md text-muted hover:bg-surface-3 hover:text-fg"
      >
        <svg viewBox="0 0 16 16" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="1.5">
          <path d="M2 4h7M12.2 4H14M2 8h3M7.8 8H14M2 12h7M12.2 12H14" strokeLinecap="round" />
          <circle cx="10.6" cy="4" r="1.5" />
          <circle cx="6.2" cy="8" r="1.5" />
          <circle cx="10.6" cy="12" r="1.5" />
        </svg>
        {updateAvailable && (
          <span
            data-update-badge
            title={t('settings.updateAvailable', { version: updateAvailable })}
            className="absolute right-1 top-1 h-1.5 w-1.5 rounded-full bg-blue-500"
          />
        )}
      </button>

      <div className="ml-1 flex h-full shrink-0 items-stretch">
        <CaptionButton label={t('caption.minimize')} onClick={() => void getCurrentWindow().minimize()}>
          <path d="M3 8h10" strokeLinecap="round" />
        </CaptionButton>
        <CaptionButton
          label={t(maximized ? 'caption.restore' : 'caption.maximize')}
          onClick={() => {
            const win = getCurrentWindow()
            void win.toggleMaximize().then(() => win.isMaximized().then(setMaximized))
          }}
        >
          {maximized ? (
            <>
              <rect x="3" y="5" width="8" height="8" rx="1.2" />
              <path d="M5.5 5V4.2A1.2 1.2 0 0 1 6.7 3h4.1A1.2 1.2 0 0 1 12 4.2v4.1" />
            </>
          ) : (
            <rect x="3.5" y="3.5" width="9" height="9" rx="1.2" />
          )}
        </CaptionButton>
        <CaptionButton label={t('caption.close')} danger onClick={() => void getCurrentWindow().close()}>
          <path d="M4 4l8 8M12 4l-8 8" strokeLinecap="round" />
        </CaptionButton>
      </div>
    </header>
  )
}
