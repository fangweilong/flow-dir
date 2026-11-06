import { useMemo } from 'react'
import type { WebdavConnection } from '../persist'
import type { Drive } from '../types'
import { WEBDAV_SCHEME } from '../paths'

/**
 * The sidebar lists one expandable row per server; the drive overview keeps only real
 * drives because capacity, eject, and shell properties do not apply to a virtual root.
 */
export function useSidebarDrives(driveList: Drive[], webdav: WebdavConnection[]): Drive[] {
  return useMemo<Drive[]>(
    () => [
      ...driveList,
      ...webdav.map((conn) => ({
        root: `${WEBDAV_SCHEME}${conn.id}/`,
        kind: 'webdav',
        name: conn.name,
        label: '',
        fs: 'WebDAV',
        total: 0,
        free: 0,
        available: true,
      })),
    ],
    [driveList, webdav],
  )
}
