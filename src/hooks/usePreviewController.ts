import { useCallback, useEffect, useRef, useState } from 'react'
import type { PointerEvent as ReactPointerEvent } from 'react'
import { previewExternal, previewInfo } from '../api'
import type { PreviewInfo } from '../types'
import type { Settings } from '../persist'
import { errorKey } from '../i18n'
import type { Translate } from '../i18n'
import type { TabState } from '../workspace'
import { joinPath } from '../paths'

const errorCode = (err: unknown): string =>
  err && typeof err === 'object' && 'code' in err && typeof err.code === 'string'
    ? err.code
    : String(err)

export interface UsePreviewControllerOptions {
  settings: Settings
  updateSettings: (next: Settings) => void
  activeTab: TabState | undefined
  previewName: string
  selectionKey: string
  previewMin: number
  showToast: (text: string) => void
  t: Translate
}

export function usePreviewController({
  settings,
  updateSettings,
  activeTab,
  previewName,
  selectionKey,
  previewMin,
  showToast,
  t,
}: UsePreviewControllerOptions) {
  const [previewOpen, setPreviewOpen] = useState(true)
  const [previewWidth, setPreviewWidth] = useState(settings.previewWidth)
  const [preview, setPreview] = useState<{
    info: PreviewInfo | null
    loading: boolean
    error: { code: string; detail?: string } | null
  }>({
    info: null,
    loading: false,
    error: null,
  })

  const previewRequest = useRef(0)

  // Preview fetch effect
  useEffect(() => {
    if (!previewOpen || !activeTab || !previewName) {
      previewRequest.current += 1
      setPreview({ info: null, loading: false, error: null })
      return
    }
    const target = joinPath(activeTab.path, previewName)
    const request = ++previewRequest.current
    setPreview({ info: null, loading: true, error: null })
    const timer = window.setTimeout(() => {
      const dark = document.documentElement.classList.contains('dark')
      const width = Math.max(64, Math.round(previewWidth - 24))
      previewInfo(target, width, dark ? '0f172a' : 'ffffff')
        .then((info) => {
          if (previewRequest.current === request) setPreview({ info, loading: false, error: null })
        })
        .catch((e: unknown) => {
          if (previewRequest.current === request) {
            const error =
              e && typeof e === 'object' && 'code' in e
                ? (e as { code: string; detail?: string })
                : { code: 'PREVIEW', detail: typeof e === 'string' ? e : '' }
            setPreview({ info: null, loading: false, error })
          }
        })
    }, 120)
    return () => window.clearTimeout(timer)
  }, [previewOpen, previewName, activeTab, settings.theme, settings.externalPreview, selectionKey, previewWidth])

  const downloadPreview = useCallback(() => {
    if (!activeTab || !previewName) return
    const target = joinPath(activeTab.path, previewName)
    const request = ++previewRequest.current
    setPreview({ info: null, loading: true, error: null })
    const dark = document.documentElement.classList.contains('dark')
    const width = Math.max(64, Math.round(previewWidth - 24))
    previewInfo(target, width, dark ? '0f172a' : 'ffffff', true)
      .then((info) => {
        if (previewRequest.current === request) setPreview({ info, loading: false, error: null })
      })
      .catch((e: unknown) => {
        if (previewRequest.current !== request) return
        const error =
          e && typeof e === 'object' && 'code' in e
            ? (e as { code: string; detail?: string })
            : { code: 'PREVIEW', detail: typeof e === 'string' ? e : '' }
        setPreview({ info: null, loading: false, error })
      })
  }, [activeTab, previewName, previewWidth])

  const openExternalPreview = useCallback(
    (target: string) => {
      void previewExternal(target).catch((err: unknown) => {
        const code = errorCode(err)
        showToast(t(code === 'NO_RULE' ? 'preview.externalMissing' : 'preview.externalFailed', { code: t(errorKey(code)) }))
      })
    },
    [showToast, t],
  )

  const startPreviewResize = useCallback(
    (ev: ReactPointerEvent<HTMLDivElement>) => {
      ev.preventDefault()
      const startX = ev.clientX
      const startWidth = previewWidth
      let finalWidth = startWidth
      const move = (e: PointerEvent) => {
        const next = Math.max(0, startWidth - (e.clientX - startX))
        if (next < previewMin) {
          if (previewOpen) setPreviewOpen(false)
          return
        }
        if (!previewOpen) setPreviewOpen(true)
        finalWidth = Math.round(next)
        setPreviewWidth(finalWidth)
      }
      const up = () => {
        window.removeEventListener('pointermove', move)
        window.removeEventListener('pointerup', up)
        window.removeEventListener('pointercancel', up)
        if (finalWidth >= previewMin) updateSettings({ ...settings, previewWidth: finalWidth })
      }
      window.addEventListener('pointermove', move)
      window.addEventListener('pointerup', up)
      window.addEventListener('pointercancel', up)
    },
    [previewMin, previewOpen, previewWidth, settings, updateSettings],
  )

  return {
    preview,
    setPreview,
    previewOpen,
    setPreviewOpen,
    previewWidth,
    setPreviewWidth,
    startPreviewResize,
    downloadPreview,
    openExternalPreview,
  }
}
