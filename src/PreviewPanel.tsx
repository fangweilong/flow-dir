import { useState } from 'react'
import { makeT } from './i18n'
import type { Translate } from './i18n'
import { formatSize, formatTime } from './paths'
import type { ApiError, Lang, PreviewInfo } from './types'
import { SvgPreview } from './components/preview/SvgPreview'
import { FontPreview } from './components/preview/FontPreview'
import { CsvPreview } from './components/preview/CsvPreview'
import { MarkdownPreview } from './components/preview/MarkdownPreview'
import { JsonPreview } from './components/preview/JsonPreview'
import { HexPreview } from './components/preview/HexPreview'
import { CodePreview } from './components/preview/CodePreview'

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
  const [htmlViewMode, setHtmlViewMode] = useState<'preview' | 'source'>('preview')
  const [binaryMode, setBinaryMode] = useState<'meta' | 'hex'>('meta')
  const [textViewMode, setTextViewMode] = useState<'text' | 'hex'>('text')
  const [imageFit, setImageFit] = useState(true)

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
            {info.kind === 'image' && info.url && (
              <div className="flex h-full flex-col">
                <div className="flex shrink-0 items-center justify-between pb-1 text-[0.625rem]">
                  <span className="font-mono text-muted-2">
                    {info.dimensions ? `${info.dimensions} px` : ''}
                  </span>
                  <button
                    type="button"
                    onClick={() => setImageFit((prev) => !prev)}
                    className="rounded bg-surface-2 px-1.5 py-0.5 text-muted hover:bg-surface-3 hover:text-fg"
                  >
                    {imageFit ? '1:1' : 'Fit'}
                  </button>
                </div>
                <div className="min-h-0 flex-1 overflow-auto grid place-items-center">
                  <img
                    src={info.url}
                    alt={name}
                    className={`rounded border border-line shadow-sm ${
                      imageFit ? 'max-h-full max-w-full object-contain' : 'max-w-none'
                    }`}
                  />
                </div>
              </div>
            )}
            {(info.kind === 'pdf' || info.kind === 'video') && info.url && (
              <div className="grid h-full place-items-center">
                <img
                  src={info.url}
                  alt={name}
                  className="max-h-full max-w-full rounded border border-line object-contain shadow-sm"
                />
              </div>
            )}
            {info.kind === 'audio' && info.url && (
              <div className="flex h-full flex-col items-center justify-center px-4 text-center">
                <svg viewBox="0 0 48 48" className="h-14 w-14 text-pink-500" fill="currentColor" aria-hidden>
                  <path d="M24 6v22.5A6.5 6.5 0 1 0 28 34V14h12V6H24Z" />
                </svg>
                <div className="mt-3 max-w-full truncate text-[0.8125rem] font-medium text-fg-strong">{name}</div>
                <audio controls src={info.url} className="mt-4 w-full max-w-xs" preload="metadata" />
              </div>
            )}

            {info.kind === 'html' && (
              <div className="flex h-full flex-col">
                <div className="flex shrink-0 items-center justify-end gap-1 pb-1.5">
                  <button
                    type="button"
                    onClick={() => setHtmlViewMode('preview')}
                    className={`rounded px-2 py-0.5 text-[0.625rem] ${
                      htmlViewMode === 'preview'
                        ? 'bg-sel font-medium text-fg-strong'
                        : 'text-muted hover:bg-surface-3'
                    }`}
                  >
                    {t('preview.htmlRender')}
                  </button>
                  <button
                    type="button"
                    onClick={() => setHtmlViewMode('source')}
                    className={`rounded px-2 py-0.5 text-[0.625rem] ${
                      htmlViewMode === 'source'
                        ? 'bg-sel font-medium text-fg-strong'
                        : 'text-muted hover:bg-surface-3'
                    }`}
                  >
                    {t('preview.htmlSource')}
                  </button>
                </div>
                {htmlViewMode === 'preview' ? (
                  <iframe
                    srcDoc={info.text ?? ''}
                    sandbox="allow-same-origin"
                    title={name}
                    className="min-h-0 flex-1 w-full rounded border border-line bg-white"
                  />
                ) : (
                  <div className="min-h-0 flex-1">
                    <CodePreview text={info.text} name={name} t={t} />
                  </div>
                )}
              </div>
            )}

            {info.kind === 'svg' && (
              <SvgPreview url={info.url} text={info.text} name={name} t={t} />
            )}

            {info.kind === 'font' && info.url && (
              <FontPreview url={info.url} name={name} mtime={info.mtime} t={t} />
            )}

            {info.kind === 'csv' && (
              <CsvPreview text={info.text} name={name} t={t} />
            )}

            {info.kind === 'markdown' && (
              <MarkdownPreview text={info.text} name={name} t={t} />
            )}

            {info.kind === 'json' && (
              <JsonPreview text={info.text} t={t} />
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
              <div className="flex h-full flex-col">
                {info.hex && (
                  <div className="flex shrink-0 items-center justify-end gap-1 pb-1.5">
                    <button
                      type="button"
                      onClick={() => setTextViewMode('text')}
                      className={`rounded px-2 py-0.5 text-[0.625rem] ${
                        textViewMode === 'text'
                          ? 'bg-sel font-medium text-fg-strong'
                          : 'text-muted hover:bg-surface-3'
                      }`}
                    >
                      {t('preview.htmlSource')}
                    </button>
                    <button
                      type="button"
                      onClick={() => setTextViewMode('hex')}
                      className={`rounded px-2 py-0.5 text-[0.625rem] ${
                        textViewMode === 'hex'
                          ? 'bg-sel font-medium text-fg-strong'
                          : 'text-muted hover:bg-surface-3'
                      }`}
                    >
                      {t('preview.hex')}
                    </button>
                  </div>
                )}
                {textViewMode === 'text' || !info.hex ? (
                  <CodePreview text={info.text} name={name} t={t} />
                ) : (
                  <HexPreview hex={info.hex} t={t} />
                )}
              </div>
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
              <div className="flex h-full flex-col">
                {info.meta && info.meta.length > 0 && info.hex && (
                  <div className="flex shrink-0 items-center justify-end gap-1 pb-1.5">
                    <button
                      type="button"
                      onClick={() => setBinaryMode('meta')}
                      className={`rounded px-2 py-0.5 text-[0.625rem] ${
                        binaryMode === 'meta'
                          ? 'bg-sel font-medium text-fg-strong'
                          : 'text-muted hover:bg-surface-3'
                      }`}
                    >
                      {t('preview.version')}
                    </button>
                    <button
                      type="button"
                      onClick={() => setBinaryMode('hex')}
                      className={`rounded px-2 py-0.5 text-[0.625rem] ${
                        binaryMode === 'hex'
                          ? 'bg-sel font-medium text-fg-strong'
                          : 'text-muted hover:bg-surface-3'
                      }`}
                    >
                      {t('preview.hex')}
                    </button>
                  </div>
                )}

                {info.meta && info.meta.length > 0 && binaryMode === 'meta' ? (
                  <div>
                    <div className="pb-1.5 text-[0.65625rem] text-muted-2">{t('preview.version')}</div>
                    <div className="space-y-1 text-[0.71875rem]">
                      {info.meta.map((row) => (
                        <div key={row.label} className="flex justify-between gap-3">
                          <span className="shrink-0 text-muted-2">{row.label}</span>
                          <span className="min-w-0 truncate text-right text-fg">{row.value}</span>
                        </div>
                      ))}
                    </div>
                  </div>
                ) : info.hex ? (
                  <HexPreview hex={info.hex} t={t} />
                ) : (
                  <div className="text-muted-2">—</div>
                )}
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
          <span>
            {info.kind === 'folder'
              ? ''
              : `${info.dimensions ? `${info.dimensions} · ` : ''}${t('preview.size')} ${formatSize(info.size, false)}`}
          </span>
          <span>{formatTime(info.mtime)}</span>
        </footer>
      )}
    </aside>
  )
}
