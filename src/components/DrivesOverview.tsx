import React from 'react'
import type { Drive } from '../types'
import type { TabState } from '../workspace'
import type { Translate } from '../i18n'
import type { MenuPoint } from '../PaneView'
import { FileIcon } from '../icons'
import { driveKindLabel } from '../i18n'
import { formatSize, isFsPath } from '../paths'

export interface DrivesOverviewProps {
  drives: Drive[]
  tab: TabState | undefined
  rowHeight: number
  driveGrid: string
  showDriveLabel: boolean
  showDriveCapacity: boolean
  listScale: number
  iconPx: number
  t: Translate
  onNavigate: (tabId: string, path: string) => void
  onOpenTarget: (path: string) => void
  onDriveRowMenu: (drive: Drive, point: MenuPoint) => void
  pointOf: (ev: React.MouseEvent) => MenuPoint
}

export function DrivesOverview({
  drives,
  tab,
  rowHeight,
  driveGrid,
  showDriveLabel,
  showDriveCapacity,
  listScale,
  iconPx,
  t,
  onNavigate,
  onOpenTarget,
  onDriveRowMenu,
  pointOf,
}: DrivesOverviewProps) {
  return (
    <div className="py-0.5">
      {drives.map((drive) => {
        const sized = drive.available && drive.total > 0
        const used = sized ? Math.max(0, drive.total - drive.free) : 0
        const usedRatio = sized ? Math.min(1, used / drive.total) : 0
        const low = sized && drive.free / drive.total < 0.1
        return (
          <div
            key={`${drive.kind}:${drive.name}`}
            data-drive={drive.root}
            onDoubleClick={() =>
              tab &&
              (isFsPath(drive.root)
                ? onNavigate(tab.id, drive.root)
                : onOpenTarget(drive.root))
            }
            onContextMenu={(ev) => {
              ev.preventDefault()
              onDriveRowMenu(drive, pointOf(ev))
            }}
            title={drive.available ? drive.root : `${drive.root} · ${t('drive.unavailable')}`}
            style={{ height: rowHeight, gridTemplateColumns: driveGrid }}
            className={`grid cursor-default items-center gap-2 border-b border-row-line px-1.5 hover:bg-surface-3 ${
              drive.available ? '' : 'text-muted-2'
            }`}
          >
            <div className="flex min-w-0 items-center gap-1.5">
              <FileIcon name={drive.name} dir path={drive.root} scale={listScale} size={iconPx} />
              <span className={`truncate font-medium ${drive.available ? 'text-fg-strong' : 'text-muted'}`}>
                {drive.name}
              </span>
            </div>
            {showDriveLabel && (
              <span className="truncate text-[0.88em] text-muted">
                {drive.label || (drive.kind === 'location' ? '' : '—')}
              </span>
            )}
            <span className="truncate text-[0.88em] text-muted-2">{driveKindLabel(t, drive.kind)}</span>
            {showDriveCapacity && (
              <>
                <div className="flex min-w-0 flex-col gap-0.5">
                  <span className="truncate text-right text-[0.8em] text-muted">
                    {sized ? `${formatSize(used, false)} / ${formatSize(drive.total, false)}` : '—'}
                  </span>
                  {sized && (
                    <div className="h-0.5 overflow-hidden rounded-full bg-surface-3">
                      <div
                        className={`h-full rounded-full ${low ? 'bg-red-500' : 'bg-blue-500'}`}
                        style={{ width: `${Math.round(usedRatio * 100)}%` }}
                      />
                    </div>
                  )}
                </div>
                <span
                  className={`truncate text-right text-[0.8em] ${low ? 'text-red-500' : 'text-muted'}`}
                >
                  {sized ? formatSize(drive.free, false) : '—'}
                </span>
              </>
            )}
          </div>
        )
      })}
    </div>
  )
}
