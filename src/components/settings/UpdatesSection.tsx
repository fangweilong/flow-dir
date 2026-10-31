import { useEffect, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { errorKey } from '../../i18n'
import type { Translate } from '../../i18n'
import type { Settings } from '../../persist'
import type { AppInfo, Lang, UpdateProgress, UpdateStatus } from '../../types'
import { taskbarProgress, updateCheck, updateInstall } from '../../api'
import { UPDATE_INTERVALS, formatBytes, ipcErrorText } from './SettingsShared'

export type UpdatesSectionProps = {
  settings: Settings
  info: AppInfo | null
  lang: Lang
  onChange: (next: Settings) => void
  t: Translate
  visible: boolean
}

export function UpdatesSection({ settings, info, lang, onChange, t, visible }: UpdatesSectionProps) {
  const [update, setUpdate] = useState<UpdateStatus | null>(null)
  const [checking, setChecking] = useState(false)
  const [installing, setInstalling] = useState(false)
  const [updateError, setUpdateError] = useState<string | null>(null)
  const [updateProgress, setUpdateProgress] = useState<UpdateProgress | null>(null)

  useEffect(() => {
    void updateCheck(false).then(setUpdate).catch(() => undefined)
  }, [])

  useEffect(() => {
    let unlisten: (() => void) | undefined
    void listen<UpdateProgress>('flowdir:update-progress', (event) => {
      setUpdateProgress(event.payload)
      if (event.payload.phase === 'ready') {
        setInstalling(false)
        void taskbarProgress('none', 0, 0).catch(() => undefined)
      } else {
        void taskbarProgress('normal', event.payload.percent, 100).catch(() => undefined)
      }
    }).then((fn) => {
      unlisten = fn
    })
    return () => {
      if (unlisten) unlisten()
    }
  }, [])

  const runUpdateCheck = () => {
    setChecking(true)
    setUpdateError(null)
    void updateCheck(true)
      .then(setUpdate)
      .catch((err: unknown) => setUpdateError(ipcErrorText(err)))
      .finally(() => setChecking(false))
  }

  const runUpdateInstall = () => {
    setInstalling(true)
    setUpdateError(null)
    setUpdateProgress({
      phase: 'connecting',
      downloaded: 0,
      total: 0,
      percent: 0,
      bytesPerSec: 0,
    })
    void updateInstall().catch((err: unknown) => {
      setInstalling(false)
      setUpdateProgress(null)
      void taskbarProgress('error', 0, 0).catch(() => undefined)
      setUpdateError(ipcErrorText(err))
    })
  }

  const updateMessage = checking
    ? t('settings.updateChecking')
    : update?.error
      ? t('settings.updateFailed', { code: t(errorKey(update.error.code)) })
      : update?.available
        ? t('settings.updateAvailable', { version: update.latest ?? '' })
        : update?.latest
          ? t('settings.updateLatest')
          : t('settings.updateIdle')
  const updateDetail = update?.error ? (update.error.detail ?? update.error.code) : undefined
  const checkedAt = update?.lastCheck
    ? new Date(update.lastCheck * 1000).toLocaleString(lang === 'zh-CN' ? 'zh-CN' : 'en-US', {
        dateStyle: 'short',
        timeStyle: 'short',
      })
    : null

  if (!visible) return null

  return (
    <section>
      <h2 className="pb-2 text-[0.6875rem] font-semibold tracking-wide text-muted-2">{t('settings.updates')}</h2>
      <div className="space-y-2 rounded-lg border border-line px-3 py-2" data-update-section>
        <div className="flex items-center justify-between gap-2">
          <div className="min-w-0">
            <div className="text-[0.75rem] text-fg">
              v{info?.version ?? update?.current ?? '…'}
              <span className="ml-2 text-[0.65625rem] text-muted-2">
                {update?.flavor === 'installer'
                  ? t('settings.updateFlavorInstaller')
                  : t('settings.updateFlavorPortable')}
              </span>
            </div>
            <div className="truncate text-[0.65625rem] text-muted-2" title={updateDetail} data-update-status>
              {updateMessage}
              {checkedAt && <span className="ml-1 text-muted-2">· {t('settings.updateLastCheck', { time: checkedAt })}</span>}
            </div>
            {update?.notes && (
              <div className="mt-0.5 truncate text-[0.65625rem] text-muted-2" title={update.notes} data-update-notes>
                {update.notes}
              </div>
            )}
          </div>
          <button
            type="button"
            onClick={runUpdateCheck}
            disabled={checking || installing}
            data-update-check
            className="shrink-0 rounded-md border border-line px-2 py-1 text-[0.6875rem] text-fg hover:bg-surface-3 disabled:opacity-40"
          >
            {t('settings.updateCheck')}
          </button>
        </div>
        {update?.available && (
          <div className="space-y-1.5">
            <button
              type="button"
              onClick={runUpdateInstall}
              disabled={installing}
              data-update-install
              className="w-full rounded-md bg-blue-600 px-2 py-1.5 text-[0.71875rem] font-medium text-white hover:bg-blue-500 disabled:opacity-60"
            >
              {installing
                ? (updateProgress?.phase === 'connecting'
                    ? t('settings.updateConnecting')
                    : updateProgress?.phase.startsWith('retrying')
                      ? t('settings.updateRetrying', { attempt: updateProgress.phase.replace('retrying (', '').replace(')', '') })
                      : updateProgress?.phase === 'verifying'
                        ? t('settings.updateVerifying')
                        : updateProgress?.phase === 'ready'
                          ? t('settings.updateReady')
                          : `${t('settings.updateDownloading')} ${updateProgress?.percent ?? 0}%`)
                : (updateError ? t('settings.updateRetry') : t('settings.updateInstall'))}
            </button>
            {installing && (
              <div className="space-y-1 rounded bg-surface-2 p-2 border border-line">
                <div className="flex items-center justify-between text-[0.6875rem]">
                  <span className="font-medium text-fg">
                    {updateProgress?.phase === 'connecting'
                      ? t('settings.updateConnecting')
                      : updateProgress?.phase.startsWith('retrying')
                        ? t('settings.updateRetrying', { attempt: updateProgress.phase.replace('retrying (', '').replace(')', '') })
                        : updateProgress?.phase === 'verifying'
                          ? t('settings.updateVerifying')
                          : updateProgress?.phase === 'ready'
                            ? t('settings.updateReady')
                            : `${updateProgress?.percent ?? 0}%`}
                  </span>
                  {updateProgress?.phase === 'downloading' && (
                    <span className="tabular text-muted-2">
                      {formatBytes(updateProgress.downloaded)} / {formatBytes(updateProgress.total)}
                      {updateProgress.bytesPerSec > 0 ? ` · ${formatBytes(updateProgress.bytesPerSec)}/s` : ''}
                    </span>
                  )}
                </div>
                <div className="h-1.5 w-full overflow-hidden rounded-full bg-surface-3">
                  <div
                    className={`h-full bg-blue-600 transition-all duration-150 ${updateProgress?.phase === 'connecting' ? 'w-1/3 animate-pulse' : ''}`}
                    style={{
                      width: updateProgress?.phase === 'connecting' ? '30%' : `${updateProgress?.percent ?? 0}%`,
                    }}
                  />
                </div>
              </div>
            )}
          </div>
        )}
        {updateError && <div className="text-[0.65625rem] text-red-500">{updateError}</div>}
        <div className="flex items-center justify-between gap-2 border-t border-line pt-2">
          <span className="text-[0.75rem] text-fg">{t('settings.updateInterval')}</span>
          <select
            value={settings.updateIntervalHours}
            onChange={(ev) => onChange({ ...settings, updateIntervalHours: Number(ev.target.value) })}
            data-update-interval
            className="h-6 rounded border border-line bg-surface-2 px-1 text-[0.6875rem] text-fg outline-none focus:border-blue-400"
          >
            {UPDATE_INTERVALS.map((hours) => (
              <option key={hours} value={hours}>
                {hours === 0
                  ? t('settings.updateManualOnly')
                  : t('settings.updateEvery', { hours })}
              </option>
            ))}
          </select>
        </div>
      </div>
    </section>
  )
}
