import { useCallback, useEffect, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { errorKey } from '../../i18n'
import type { Translate } from '../../i18n'
import type { Settings } from '../../persist'
import type { AppInfo, Lang, ReleaseEntry, UpdateProgress, UpdateStatus } from '../../types'
import { openUrl, taskbarProgress, updateCheck, updateHistory, updateInstall } from '../../api'
import { UPDATE_INTERVALS, formatBytes, ipcErrorText } from './SettingsShared'

export type UpdatesSectionProps = {
  settings: Settings
  info: AppInfo | null
  lang: Lang
  onChange: (next: Settings) => void
  t: Translate
  visible: boolean
}

/** GitHub's generated notes are markdown; drop the markers that would otherwise show as
 * syntax. HTML is never injected, so arbitrary release text stays inert. */
function readableNotes(notes: string): string {
  return notes
    .replace(/\r\n/g, '\n')
    .replace(/^#{1,6}\s+/gm, '')
    .replace(/^\s*[-*]\s+/gm, '• ')
    .replace(/\*\*(.+?)\*\*/g, '$1')
    .replace(/`([^`]+)`/g, '$1')
    .trim()
}

export function UpdatesSection({ settings, info, lang, onChange, t, visible }: UpdatesSectionProps) {
  const [update, setUpdate] = useState<UpdateStatus | null>(null)
  const [checking, setChecking] = useState(false)
  const [installing, setInstalling] = useState(false)
  const [updateError, setUpdateError] = useState<string | null>(null)
  const [updateProgress, setUpdateProgress] = useState<UpdateProgress | null>(null)
  const [history, setHistory] = useState<ReleaseEntry[]>([])
  const [historyLoading, setHistoryLoading] = useState(false)
  const [historyError, setHistoryError] = useState<string | null>(null)

  const loadHistory = useCallback((force: boolean) => {
    setHistoryLoading(true)
    setHistoryError(null)
    void updateHistory(force)
      .then(setHistory)
      .catch((err: unknown) => setHistoryError(ipcErrorText(err)))
      .finally(() => setHistoryLoading(false))
  }, [])

  useEffect(() => {
    void updateCheck(false).then(setUpdate).catch(() => undefined)
  }, [])

  // Only fetch when the (merged) About tab is actually shown; opening Settings must not
  // spend GitHub API quota on a page the user never looks at.
  useEffect(() => {
    if (visible) loadHistory(false)
  }, [visible, loadHistory])

  useEffect(() => {
    let disposed = false
    let unlistenProgress: (() => void) | undefined
    let unlistenUpdate: (() => void) | undefined
    void listen<UpdateProgress>('flowdir:update-progress', (event) => {
      setUpdateProgress(event.payload)
      if (event.payload.phase === 'ready') {
        setInstalling(false)
        void taskbarProgress('none', 0, 0).catch(() => undefined)
      } else {
        void taskbarProgress('normal', event.payload.percent, 100).catch(() => undefined)
      }
    }).then((fn) => {
      // The settings panel can close before `listen` resolves; the handler then keeps a
      // dead component subscribed unless it is released here.
      if (disposed) fn()
      else unlistenProgress = fn
    })
    // The background checker announces releases it finds on this channel. Without it a
    // page that was already open when the check finished kept showing the old cached
    // status (and no install button) until the user pressed the manual check button.
    void listen<UpdateStatus>('flowdir:update', (event) => setUpdate(event.payload)).then((fn) => {
      if (disposed) fn()
      else unlistenUpdate = fn
    })
    return () => {
      disposed = true
      unlistenProgress?.()
      unlistenUpdate?.()
    }
  }, [])

  const runUpdateCheck = () => {
    setChecking(true)
    setUpdateError(null)
    void updateCheck(true)
      .then((next) => {
        setUpdate(next)
        // The manual check is also the explicit refresh path for the release history.
        loadHistory(true)
      })
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
  const releaseDate = (iso: string | null) => {
    if (!iso) return ''
    const date = new Date(iso)
    if (Number.isNaN(date.getTime())) return ''
    return date.toLocaleDateString(lang === 'zh-CN' ? 'zh-CN' : 'en-US', { dateStyle: 'medium' })
  }

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
              className="w-full rounded-md bg-accent-strong px-2 py-1.5 text-[0.71875rem] font-medium text-white hover:bg-accent disabled:opacity-60"
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
                    className={`h-full bg-accent-strong transition-all duration-150 ${updateProgress?.phase === 'connecting' ? 'w-1/3 animate-pulse' : ''}`}
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
            className="h-6 rounded border border-line bg-surface-2 px-1 text-[0.6875rem] text-fg outline-none focus:border-accent-soft"
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
        <div className="border-t border-line pt-2" data-update-history>
          <div className="flex items-center justify-between gap-2 pb-1">
            <span className="text-[0.71875rem] text-fg">{t('settings.updateHistory')}</span>
            {historyLoading && (
              <span className="text-[0.625rem] text-muted-2">{t('settings.updateHistoryLoading')}</span>
            )}
          </div>
          {historyError && (
            <div className="text-[0.625rem] text-red-500">
              {t('settings.updateHistoryFailed')}: {historyError}
            </div>
          )}
          {!history.length && !historyLoading && !historyError && (
            <div className="text-[0.625rem] text-muted-2">{t('settings.updateHistoryEmpty')}</div>
          )}
          {history.length > 0 && (
            <div className="max-h-56 space-y-1 overflow-y-auto pr-1">
              {history.map((release) => (
                <details
                  key={`${release.version}-${release.publishedAt ?? ''}`}
                  className="rounded border border-line bg-surface-2/40"
                >
                  <summary className="flex cursor-pointer list-none items-center gap-2 px-2 py-1">
                    <span className="font-mono text-[0.6875rem] text-fg">v{release.version}</span>
                    {release.version === info?.version && (
                      <span className="rounded bg-accent/15 px-1 py-px text-[0.59375rem] text-accent-strong dark:text-accent-soft">
                        {t('settings.updateCurrentVersion')}
                      </span>
                    )}
                    {release.prerelease && (
                      <span className="rounded bg-warning/15 px-1 py-px text-[0.59375rem] text-warning-strong dark:text-warning-soft">
                        {t('settings.prerelease')}
                      </span>
                    )}
                    <span className="ml-auto shrink-0 text-[0.625rem] text-muted-2">
                      {releaseDate(release.publishedAt)}
                    </span>
                  </summary>
                  <div className="space-y-1 border-t border-line px-2 py-1.5">
                    {release.notes ? (
                      <div className="max-h-40 overflow-y-auto whitespace-pre-wrap break-words text-[0.65625rem] leading-relaxed text-muted">
                        {readableNotes(release.notes)}
                      </div>
                    ) : (
                      <div className="text-[0.625rem] text-muted-2">{t('settings.updateNoNotes')}</div>
                    )}
                    {release.url && (
                      <button
                        type="button"
                        onClick={() => void openUrl(release.url!).catch(() => undefined)}
                        className="rounded px-1 py-0.5 text-[0.625rem] text-accent-strong hover:bg-surface-3 dark:text-accent-soft"
                      >
                        {t('settings.releasePage')}
                      </button>
                    )}
                  </div>
                </details>
              ))}
            </div>
          )}
        </div>
      </div>
    </section>
  )
}
