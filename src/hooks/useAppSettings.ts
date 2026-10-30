import { useCallback, useEffect, useRef, useState } from 'react'
import type { MutableRefObject } from 'react'
import { listen } from '@tauri-apps/api/event'
import {
  configWarnings,
  drives,
  layoutLoad,
  layoutPreserve,
  quickAccess,
  setAppTheme,
  settingsLoad,
  settingsSave,
  updateCheck,
} from '../api'
import type { Drive, Place, UpdateStatus } from '../types'
import { makeT } from '../i18n'
import {
  DEFAULT_LIST_FONT_SIZE,
  DEFAULT_NAV_FONT_SIZE,
  DEFAULT_TAB_FONT_SIZE,
  clampFontSize,
  clampRegionFontSize,
  fontStack,
} from '../fonts'
import { DEFAULT_SETTINGS, fromSnapshot } from '../persist'
import type { Settings } from '../persist'
import { themeIsDark } from '../themes'
import { adoptIds, createWorkspace } from '../workspace'
import type { Workspace } from '../workspace'

const errorCode = (err: unknown): string =>
  err && typeof err === 'object' && 'code' in err && typeof err.code === 'string'
    ? err.code
    : String(err)

export interface UseAppSettingsOptions {
  showToast: (text: string, ms?: number) => void
  setWs: React.Dispatch<React.SetStateAction<Workspace | null>>
  setPlaces: React.Dispatch<React.SetStateAction<Place[]>>
  setDriveList: React.Dispatch<React.SetStateAction<Drive[]>>
  homeRef: MutableRefObject<string>
  readyRef: MutableRefObject<boolean>
  settingsRef: MutableRefObject<Settings>
  sidebarMin: number
  previewMin: number
  setPreviewOpen: React.Dispatch<React.SetStateAction<boolean>>
  setSidebarWidth: React.Dispatch<React.SetStateAction<number>>
  setPreviewWidth: React.Dispatch<React.SetStateAction<number>>
}

export function useAppSettings({
  showToast,
  setWs,
  setPlaces,
  setDriveList,
  homeRef,
  readyRef,
  settingsRef,
  sidebarMin,
  previewMin,
  setPreviewOpen,
  setSidebarWidth,
  setPreviewWidth,
}: UseAppSettingsOptions) {
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS)
  const settingsSaveTimer = useRef<number | null>(null)
  const pendingSettings = useRef<Settings | null>(null)
  const lastSaveError = useRef(0)
  const [updateAvailable, setUpdateAvailable] = useState<string | null>(null)
  const notifiedUpdate = useRef<string | null>(null)
  const bootstrappedRef = useRef(false)
  const optionsRef = useRef({
    showToast,
    setWs,
    setPlaces,
    setDriveList,
    homeRef,
    readyRef,
    sidebarMin,
    previewMin,
    setPreviewOpen,
    setSidebarWidth,
    setPreviewWidth,
  })
  optionsRef.current = {
    showToast,
    setWs,
    setPlaces,
    setDriveList,
    homeRef,
    readyRef,
    sidebarMin,
    previewMin,
    setPreviewOpen,
    setSidebarWidth,
    setPreviewWidth,
  }
  useEffect(() => {
    settingsRef.current = settings
  }, [settings])

  const reportSaveError = useCallback(
    (err: unknown) => {
      const now = Date.now()
      if (now - lastSaveError.current < 10_000) return
      lastSaveError.current = now
      showToast(makeT(settingsRef.current.language)('toast.saveFailed', { code: errorCode(err) }))
    },
    [showToast],
  )

  const reportIntegrationWarnings = useCallback(
    (codes: string[]) => {
      if (!codes.length) return
      const tr = makeT(settingsRef.current.language)
      const labels = codes.map((code) => {
        if (code === 'HOTKEY_CONFLICT') return tr('settings.hotkeyConflict')
        if (code === 'HOTKEY_INVALID') return tr('settings.hotkeyInvalid')
        return tr('settings.integrationFailed')
      })
      showToast(tr('toast.integrationWarnings', { list: labels.join('; ') }))
    },
    [showToast],
  )

  const updateSettings = useCallback((next: Settings) => {
    setSettings(next)
    settingsRef.current = next
    pendingSettings.current = next
    if (settingsSaveTimer.current !== null) window.clearTimeout(settingsSaveTimer.current)
    settingsSaveTimer.current = window.setTimeout(() => {
      settingsSaveTimer.current = null
      const snapshot = pendingSettings.current
      pendingSettings.current = null
      if (snapshot) {
        void settingsSave(snapshot).then(reportIntegrationWarnings).catch(reportSaveError)
      }
    }, 300)
  }, [reportIntegrationWarnings, reportSaveError])

  const flushSettings = useCallback(() => {
    if (settingsSaveTimer.current !== null) {
      window.clearTimeout(settingsSaveTimer.current)
      settingsSaveTimer.current = null
    }
    const snapshot = pendingSettings.current
    pendingSettings.current = null
    if (snapshot) {
      void settingsSave(snapshot).then(reportIntegrationWarnings).catch(reportSaveError)
    }
  }, [reportIntegrationWarnings, reportSaveError])

  // Bootstrap initial settings & workspace
  useEffect(() => {
    if (bootstrappedRef.current) return
    bootstrappedRef.current = true
    let cancelled = false
    void (async () => {
      const {
        showToast: toast,
        setWs: updateWs,
        setPlaces: updatePlaces,
        setDriveList: updateDrives,
        homeRef: home,
        readyRef: ready,
        sidebarMin: sMin,
        previewMin: pMin,
        setPreviewOpen: updatePreviewOpen,
        setSidebarWidth: updateSidebarWidth,
        setPreviewWidth: updatePreviewWidth,
      } = optionsRef.current
      const [savedSettings, placeList, driveListResult, saved, warnings] = await Promise.all([
        settingsLoad().catch(() => DEFAULT_SETTINGS),
        quickAccess().catch(() => [] as Place[]),
        drives().catch(() => [] as Drive[]),
        layoutLoad().catch(() => null),
        configWarnings().catch(() => [] as string[]),
      ])
      if (cancelled) return
      const merged = { ...DEFAULT_SETTINGS, ...savedSettings }
      if (warnings.includes('SETTINGS_REJECTED')) {
        toast(makeT(merged.language)('toast.settingsRejected'))
      }
      if (warnings.includes('LAYOUT_REJECTED')) {
        toast(makeT(merged.language)('toast.layoutRejected'))
      }
      setSettings(merged)
      updatePreviewOpen(merged.previewOpen)
      updateSidebarWidth(merged.sidebarWidth >= sMin ? merged.sidebarWidth : DEFAULT_SETTINGS.sidebarWidth)
      updatePreviewWidth(merged.previewWidth >= pMin ? merged.previewWidth : DEFAULT_SETTINGS.previewWidth)
      updatePlaces(placeList)
      updateDrives(driveListResult)
      const homeDir = placeList[0]?.path ?? 'C:\\'
      home.current = homeDir
      const restored = fromSnapshot(saved)
      if (saved && !restored) void layoutPreserve().catch(() => undefined)
      const initial =
        (merged.restoreTabsOnStart ? restored : null) ?? createWorkspace(homeDir, merged.showHidden)
      adoptIds(initial)
      updateWs(initial)
      ready.current = true
    })()
    return () => {
      cancelled = true
    }
  }, [])

  // Font styling
  useEffect(() => {
    const root = document.documentElement
    root.style.setProperty('--font-ui', fontStack(settings.fontFamily))
    root.style.setProperty('--font-size-base', `${clampFontSize(settings.fontSize)}px`)
    const regions = [
      ['tab', settings.tabFontFamily, clampRegionFontSize(settings.tabFontSize, DEFAULT_TAB_FONT_SIZE)],
      ['list', settings.listFontFamily, clampRegionFontSize(settings.listFontSize, DEFAULT_LIST_FONT_SIZE)],
      ['nav', settings.navFontFamily, clampRegionFontSize(settings.navFontSize, DEFAULT_NAV_FONT_SIZE)],
    ] as const
    for (const [region, family, size] of regions) {
      root.style.setProperty(`--font-size-${region}`, `${size}px`)
      if (family.trim()) root.style.setProperty(`--font-${region}`, fontStack(family))
      else root.style.removeProperty(`--font-${region}`)
    }
  }, [
    settings.fontFamily,
    settings.fontSize,
    settings.tabFontFamily,
    settings.tabFontSize,
    settings.listFontFamily,
    settings.listFontSize,
    settings.navFontFamily,
    settings.navFontSize,
  ])

  // Theme styling
  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const apply = () => {
      const root = document.documentElement
      const dark = themeIsDark(settings.theme, media.matches)
      if (settings.theme === 'system') delete root.dataset.theme
      else root.dataset.theme = settings.theme
      root.classList.toggle('dark', dark)
      root.style.colorScheme = dark ? 'dark' : 'light'
    }
    apply()
    void setAppTheme(settings.theme).catch(() => undefined)
    media.addEventListener('change', apply)
    return () => media.removeEventListener('change', apply)
  }, [settings.theme])

  // Column width handlers
  const setColWidths = useCallback(
    (size: number, time: number, type: number, capacity: number, free: number) => {
      updateSettings({
        ...settingsRef.current,
        listSizeWidth: size,
        listTimeWidth: time,
        listTypeWidth: type,
        driveCapacityWidth: capacity,
        driveFreeWidth: free,
      })
    },
    [updateSettings],
  )

  const setNameWidth = useCallback(
    (width: number) => {
      updateSettings({ ...settingsRef.current, listNameWidth: width })
    },
    [updateSettings],
  )

  useEffect(() => {
    const announce = (status: UpdateStatus) => {
      if (!status.available || !status.latest) return
      setUpdateAvailable(status.latest)
      if (notifiedUpdate.current === status.latest) return
      notifiedUpdate.current = status.latest
      showToast(makeT(settingsRef.current.language)('toast.updateAvailable', { version: status.latest }))
    }
    let disposed = false
    let unlisten: (() => void) | undefined
    void listen<UpdateStatus>('flowdir:update', (event) => announce(event.payload)).then((fn) => {
      if (disposed) fn()
      else unlisten = fn
    })
    const timer = window.setTimeout(() => {
      void updateCheck(false)
        .then((status) => {
          if (!disposed) announce(status)
        })
        .catch(() => undefined)
    }, 3000)
    return () => {
      disposed = true
      unlisten?.()
      window.clearTimeout(timer)
    }
  }, [settingsRef, showToast])

  return {
    settings,
    setSettings,
    settingsRef,
    updateSettings,
    flushSettings,
    reportSaveError,
    reportIntegrationWarnings,
    setColWidths,
    setNameWidth,
    updateAvailable,
    setUpdateAvailable,
  }
}
