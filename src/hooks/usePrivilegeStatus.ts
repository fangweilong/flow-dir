import { useCallback, useEffect, useState } from 'react'
import { isElevated, restartPrivilege, uacEnabled } from '../api'
import { errorCode } from '../errorCode'
import { errorKey } from '../i18n'
import type { Translate } from '../i18n'

/**
 * Integrity level is fixed for the process lifetime; switching restarts the app, so this
 * only reads the current state and requests a relaunch.
 */
export function usePrivilegeStatus(
  showToast: (msg: string, timeout?: number) => void,
  t: Translate,
) {
  const [elevated, setElevated] = useState(false)
  const [uacOn, setUacOn] = useState(true)

  useEffect(() => {
    void isElevated().then(setElevated).catch(() => undefined)
    void uacEnabled().then(setUacOn).catch(() => undefined)
  }, [])

  const switchPrivilege = useCallback(
    (wanted: boolean) => {
      void restartPrivilege(wanted).catch((e: unknown) => {
        const code = errorCode(e)
        showToast(
          code === 'ELEVATION_CANCELED'
            ? t('priv.canceled')
            : t('priv.failed', { code: t(errorKey(code)) }),
        )
      })
    },
    [showToast, t],
  )

  return { elevated, uacOn, switchPrivilege }
}
