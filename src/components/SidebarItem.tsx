import React from 'react'

export interface SidebarItemProps {
  label: string
  hint?: string
  active: boolean
  icon: React.ReactNode
  /** Folder that accepts dragged files; unset makes the row an inert drop target. */
  dropDir?: string
  onClick: () => void
  onContextMenu?: (ev: React.MouseEvent<HTMLButtonElement>) => void
}

export function SidebarItem({
  label,
  hint,
  active,
  icon,
  dropDir,
  onClick,
  onContextMenu,
}: SidebarItemProps) {
  return (
    <button
      type="button"
      data-drop-dir={dropDir}
      onClick={onClick}
      onContextMenu={onContextMenu}
      className={`flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-[0.75rem] ${
        active ? 'bg-sel font-medium text-fg-strong' : 'text-fg hover:bg-surface-3'
      }`}
    >
      {icon}
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {hint && <span className="shrink-0 text-[0.625rem] text-muted-2">{hint}</span>}
    </button>
  )
}
