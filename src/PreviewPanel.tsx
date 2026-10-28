import { makeT } from './i18n'
import type { Translate } from './i18n'
import { formatSize, formatTime } from './paths'
import type { ApiError, Lang, PreviewInfo } from './types'

type Props = {
  info: PreviewInfo | null
  loading: boolean
  error: ApiError | null
  name: string
  path: string
  lang: Lang
  /** Drag-resized width in px. */
  width: number
  /** CSS cap that keeps the centre panes at least half of the layout. */
  maxWidth: string
  /** Bound key for the external previewer, shown on the button. */
  externalKey: string
  onClose: () => void
  onOpenExternal: (path: string) => void
  /** Reads the cloud file, which downloads it; only ever called from an explicit click. */
  onDownload: () => void
}

/** Maps raw error codes/details to a human sentence; never prints `[object Object]`. */
function friendlyError(error: ApiError, t: Translate) {
  switch (error.code) {
    case 'ACCESS_DENIED':
    case 'WINDOWS':
      return t('preview.errorAccess')
    case 'TOO_LARGE':
      return t('preview.tooLarge')
    case 'CLOUD_PLACEHOLDER':
      return t('preview.cloud')
    case 'IS_DIR':
      return t('preview.folder')
    case 'NOT_FOUND':
      return t('preview.errorMissing')
    default:
      return t('preview.errorGeneric')
  }
}

export function PreviewPanel({
  info,
  loading,
  error,
  name,
  path,
  lang,
  width,
  maxWidth,
  externalKey,
  onClose,
  onOpenExternal,
  onDownload,
}: Props) {
  const t = makeT(lang)

  return (
    <aside
      style={{ width, maxWidth }}
      className="flex shrink-0 flex-col overflow-x-hidden border-l border-line bg-surface"
    >
      <header className="flex h-7 shrink-0 items-center gap-2 border-b border-line px-2">
        <span className="text-[0.625rem] font-semibold tracking-wider text-muted-2">{t('preview.title')}</span>
        <span className="min-w-0 flex-1 truncate text-[0.71875rem] font-medium text-fg-strong" title={name}>
          {name}
        </span>
        <button
          type="button"
          onClick={onClose}
          className="grid h-5 w-5 place-items-center rounded text-muted-2 hover:bg-surface-3 hover:text-fg"
          title={t('settings.close')}
        >
          ×
        </button>
      </header>

      <div className="relative min-h-0 flex-1 overflow-auto p-2">
        {loading && (
          <div className="grid h-full place-items-center text-[0.71875rem] text-muted-2">{t('preview.loading')}</div>
        )}

        {!loading && error && (
          <div className="grid h-full place-items-center px-4 text-center">
            <div>
              <div className="text-[0.75rem] text-fg">{friendlyError(error, t)}</div>
              <div className="mt-1 font-mono text-[0.625rem] text-muted-2">{error.code}</div>
            </div>
          </div>
        )}

        {!loading && !error && !info && (
          <div className="grid h-full place-items-center text-center text-[0.71875rem] text-muted-2">
            {t('preview.empty')}
          </div>
        )}

        {!loading && !error && info && (
          <>
            {(info.kind === 'image' || info.kind === 'pdf' || info.kind === 'video') && info.url && (
              <div className="grid h-full place-items-center">
                <img
                  src={info.url}
                  alt={name}
                  className="max-h-full max-w-full rounded border border-line object-contain shadow-sm"
                />
              </div>
            )}

            {info.kind === 'folder' && (
              <div className="grid h-full place-items-center px-4 text-center">
                <div>
                  <svg viewBox="0 0 48 48" className="mx-auto h-12 w-12 text-amber-400" fill="currentColor" aria-hidden>
                    <path d="M4 12a4 4 0 0 1 4-4h11l4 5h17a4 4 0 0 1 4 4v19a4 4 0 0 1-4 4H8a4 4 0 0 1-4-4V12Z" />
                  </svg>
                  <div className="mt-2 text-[0.75rem] font-medium text-fg-strong">{t('preview.folder')}</div>
                  <div className="mt-1 text-[0.6875rem] text-muted-2">{t('preview.folderHint')}</div>
                </div>
              </div>
            )}

            {info.kind === 'text' && (
              <pre data-selectable className="whitespace-pre-wrap break-words font-mono text-[0.6875rem] leading-relaxed text-fg">
                {info.text}
              </pre>
            )}

            {info.kind === 'archive' && (
              <div>
                <div className="pb-1.5 text-[0.65625rem] text-muted-2">{t('preview.archive')}</div>
                <div className="space-y-0.5 font-mono text-[0.6875rem]">
                  {(info.entries ?? []).map((entry) => (
                    <div key={entry.name} className="flex justify-between gap-3">
                      <span className="min-w-0 truncate text-fg">{entry.name}</span>
                      <span className="shrink-0 text-muted-2">{formatSize(entry.size, false)}</span>
                    </div>
                  ))}
                  {!info.entries?.length && <div className="text-muted-2">—</div>}
                </div>
              </div>
            )}

            {info.kind === 'binary' && (
              <div>
                <div className="pb-1.5 text-[0.65625rem] text-muted-2">{t('preview.version')}</div>
                <div className="space-y-1 text-[0.71875rem]">
                  {(info.meta ?? []).map((row) => (
                    <div key={row.label} className="flex justify-between gap-3">
                      <span className="shrink-0 text-muted-2">{row.label}</span>
                      <span className="min-w-0 truncate text-right text-fg">{row.value}</span>
                    </div>
                  ))}
                  {!info.meta?.length && <div className="text-muted-2">—</div>}
                </div>
              </div>
            )}

            {info.kind === 'cloud' && (
              <div className="grid h-full place-items-center px-4 text-center">
                <div>
                  <svg viewBox="0 0 48 48" className="mx-auto h-11 w-11 text-sky-400" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden>
                    <path d="M14 34h20a8 8 0 0 0 1.6-15.8A11 11 0 0 0 14.5 16 9 9 0 0 0 14 34Z" />
                    <path d="M24 24v10M20 30l4 4 4-4" />
                  </svg>
                  <div className="mt-2 text-[0.75rem] text-fg">{t('preview.cloud')}</div>
                  <div className="mt-1 text-[0.6875rem] text-muted-2">{t('preview.cloudHint')}</div>
                  <button
                    type="button"
                    data-preview-download
                    onClick={onDownload}
                    className="mt-3 rounded-md border border-line bg-surface-2 px-2.5 py-1 text-[0.6875rem] text-fg hover:bg-surface-3"
                  >
                    {t('preview.cloudDownload')}
                  </button>
                </div>
              </div>
            )}

            {info.kind === 'unsupported' && (
              <div className="grid h-full place-items-center px-4 text-center">
                <div>
                  <svg viewBox="0 0 48 48" className="mx-auto h-11 w-11 text-muted-2" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden>
                    <path d="M12 6h16l8 8v28a2 2 0 0 1-2 2H12a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2Z" />
                    <path d="M28 6v8h8" />
                  </svg>
                  <div className="mt-2 text-[0.75rem] text-fg">{t('preview.unsupported')}</div>
                  <div className="mt-1 text-[0.6875rem] text-muted-2">
                    {info.reason === 'too-large' ? t('preview.tooLarge') : t('preview.noPreview')}
                  </div>
                  {info.external && (
                    <button
                      type="button"
                      onClick={() => onOpenExternal(path)}
                      title={externalKey}
                      className="mt-3 rounded-md border border-line bg-surface-2 px-2.5 py-1 text-[0.6875rem] text-fg hover:bg-surface-3"
                    >
                      {t('preview.external', { name: info.external })}
                    </button>
                  )}
                </div>
              </div>
            )}
          </>
        )}
      </div>

      {info && (
        <footer className="flex h-7 shrink-0 items-center justify-between border-t border-line px-2.5 text-[0.65625rem] text-muted-2">
          <span>{info.kind === 'folder' ? '' : `${t('preview.size')} ${formatSize(info.size, false)}`}</span>
          <span>{formatTime(info.mtime)}</span>
        </footer>
      )}
    </aside>
  )
}
