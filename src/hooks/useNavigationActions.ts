import { useCallback } from 'react'
import type { MutableRefObject } from 'react'
import { openPath, openWith } from '../api'
import { errorCode } from '../errorCode'
import { errorKey } from '../i18n'
import type { Translate } from '../i18n'
import {
  isFsPath,
  isInsideArchive,
  isShellPath,
  isWebdavPath,
} from '../paths'
import type { Workspace } from '../workspace'
import { refreshIconCache } from '../icons'

export interface UseNavigationActionsOptions {
  wsRef: MutableRefObject<Workspace | null>
  navigate: (tabId: string, path: string) => void
  showToast: (msg: string, timeout?: number) => void
  t: Translate
  /** Runs before an in-app navigation (the settings modal closes itself this way). */
  beforeOpen?: () => void
}

/** Opening targets and the chrome that needs to resolve them against the active tab. */
export function useNavigationActions({
  wsRef,
  navigate,
  showToast,
  t,
  beforeOpen,
}: UseNavigationActionsOptions) {
  const openTarget = useCallback(
    (targetPath: string) => {
      openPath(targetPath)
        .then(() => {
          if (isInsideArchive(targetPath)) refreshIconCache()
        })
        .catch((err: unknown) => {
          const code = errorCode(err)
          if (code === 'NO_ASSOCIATION') {
            // A remote file has no local path; the backend's error detail carries the
            // cached copy it downloaded, which is what "Open with" must hand to the shell.
            const detail =
              isWebdavPath(targetPath) &&
              err &&
              typeof err === 'object' &&
              'detail' in err &&
              typeof err.detail === 'string'
                ? err.detail
                : ''
            void openWith(detail || targetPath).catch((e: unknown) => {
              showToast(t('op.failed', { code: t(errorKey(errorCode(e))) }))
            })
            return
          }
          showToast(t('op.failed', { code: t(errorKey(code)) }))
        })
    },
    [showToast, t],
  )

  /** Opens a folder in the active tab, used by the go-to-path field and settings links. */
  const openInApp = useCallback(
    (path: string) => {
      beforeOpen?.()
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[current.activePaneId] ?? Object.values(current.panes)[0]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (!tab) return
      navigate(tab.id, path)
    },
    [beforeOpen, navigate, wsRef],
  )

  /** Tree/sidebar targets: navigable namespaces stay in-app, everything else goes to the shell. */
  const openTreePath = useCallback(
    (path: string) => {
      const current = wsRef.current
      if (!current) return
      const pane = current.panes[current.activePaneId]
      const tab = pane ? current.tabs[pane.activeTabId] : undefined
      if (!tab) return
      if (!isFsPath(path) && !isShellPath(path) && !isWebdavPath(path)) {
        openTarget(path)
        return
      }
      navigate(tab.id, path)
    },
    [navigate, openTarget, wsRef],
  )

  return { openTarget, openInApp, openTreePath }
}
