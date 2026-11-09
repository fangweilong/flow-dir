import { useCallback, useEffect, useRef, useState } from 'react'
import type { MutableRefObject } from 'react'
import { listen } from '@tauri-apps/api/event'
import {
  configWarnings,
  drives,
  layoutLoad,
  layoutPreserve,
  layoutUnreadable,
  quickAccess,
  setAppTheme,
  settingsLoad,
  settingsSave,
  settingsUnreadable,
  updateCheck,
} from '../api'
import type { Drive, Place, UpdateStatus } from '../types'
import { errorKey, makeT } from '../i18n'
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
import { applyIconTheme } from '../iconThemes'
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
  /** Stops layout autosaves after `layout_load` fell back to a backup/default. */
  layoutLockedRef: MutableRefObject<boolean>
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
  layoutLockedRef,
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
  // A load that could not read the real file must not be written back: the snapshot is
  // defaults (or a stale backup), so a save would replace every field the user had.
  const settingsLocked = useRef(false)
  // Settings changed while `settings_load` is still in flight. The bootstrap merge replays
  // them over the loaded file: sending them before the load would write defaults, and
  // dropping them would lose the user's first toggles.
  const earlyPatch = useRef<Partial<Settings>>({})
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
    layoutLockedRef,
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
    layoutLockedRef,
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
      const tr = makeT(settingsRef.current.language)
      showToast(tr('toast.saveFailed', { code: tr(errorKey(errorCode(err))) }))
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
    const previous = settingsRef.current
    setSettings(next)
    settingsRef.current = next
    // Until the bootstrap load resolves, `settings` is still DEFAULT_SETTINGS; saving a
    // change now would write those defaults over every field already on disk. Record only
    // the fields this call changed, for the merge to apply after the load.
    if (!optionsRef.current.readyRef.current) {
      const patch = earlyPatch.current as Record<string, unknown>
      for (const key of Object.keys(next)) {
        if ((next as Record<string, unknown>)[key] !== (previous as Record<string, unknown>)[key]) {
          patch[key] = (next as Record<string, unknown>)[key]
        }
      }
      return
    }
    if (settingsLocked.current) return
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
    if (settingsLocked.current) return
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
        layoutLockedRef: lockLayout,
        sidebarMin: sMin,
        previewMin: pMin,
        setSidebarWidth: updateSidebarWidth,
        setPreviewWidth: updatePreviewWidth,
      } = optionsRef.current
      // The unreadable flag is read after `settings_load` on purpose: the two IPCs must
      // describe the same load, and the flag is set while that call reads the file.
      const loaded = settingsLoad()
        .then(async (value) => ({
          value,
          unreadable: await settingsUnreadable().catch(() => false),
        }))
        .catch(() => ({ value: DEFAULT_SETTINGS, unreadable: false }))
      const [savedSettings, placeList, driveListResult, saved, warnings, layoutUnreadableFlag] =
        await Promise.all([
          loaded,
          quickAccess().catch(() => [] as Place[]),
          drives().catch(() => [] as Drive[]),
          layoutLoad().catch(() => null),
          configWarnings().catch(() => [] as string[]),
          // Sticky on the backend: unlike the one-shot warning code, this survives the
          // `config_warnings` read racing ahead of `layout_load`.
          layoutUnreadable().catch(() => false),
        ])
      if (cancelled) return
      const early = earlyPatch.current
      const merged =
        Object.keys(early).length > 0
          ? { ...DEFAULT_SETTINGS, ...savedSettings.value, ...early }
          : { ...DEFAULT_SETTINGS, ...savedSettings.value }
      if (layoutUnreadableFlag) {
        // The workspace in memory came from a backup (or defaults); keep it readable but
        // never overwrite the primary file with this run's edits.
        lockLayout.current = true
        if (!warnings.includes('LAYOUT_UNREADABLE')) {
          toast(makeT(merged.language)('toast.layoutUnreadable'), 12_000)
        }
      }
      if (savedSettings.unreadable) {
        // Keep the window usable but never write this run: `settings_load` returned
        // defaults/backup because the real file was inaccessible.
        settingsLocked.current = true
        toast(makeT(merged.language)('toast.settingsUnreadable'), 12_000)
      }
      if (warnings.includes('SETTINGS_REJECTED')) {
        toast(makeT(merged.language)('toast.settingsRejected'))
      }
      if (warnings.includes('LAYOUT_REJECTED')) {
        toast(makeT(merged.language)('toast.layoutRejected'))
      }
      if (warnings.includes('LAYOUT_UNREADABLE')) {
        toast(makeT(merged.language)('toast.layoutUnreadable'))
      }
      setSettings(merged)
      // `previewOpen` is applied by `usePreviewController`'s own effect from `settings`.
      // Writing it through `setPreviewOpen` here used a controller that still held
      // DEFAULT_SETTINGS, so launching with the panel closed saved every settings field
      // as its default (and cleared the shell integration).
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
      // Changes made while the load was in flight changed `settingsRef` but were not
      // saved; replay them now that the loaded values are the base.
      if (Object.keys(early).length > 0) updateSettings(merged)
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
      applyIconTheme(settings.iconTheme, dark)
    }
    apply()
    void setAppTheme(settings.theme).catch(() => undefined)
    media.addEventListener('change', apply)
    return () => media.removeEventListener('change', apply)
  }, [settings.theme, settings.iconTheme])

  // Column width handlers
  const setColWidths = useCallback(
    (size: number, type: number, time: number, capacity: number, free: number) => {
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
