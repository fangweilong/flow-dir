import { useState } from 'react'
import type { Translate } from '../../i18n'

type Props = {
  url: string | null | undefined
  text: string | null | undefined
  name: string
  t: Translate
}

export function SvgPreview({ url, text, name, t }: Props) {
  const [mode, setMode] = useState<'preview' | 'source'>('preview')
  const [copied, setCopied] = useState(false)

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between pb-1.5">
        <div className="flex items-center gap-1">
          {text && (
            <button
              type="button"
              onClick={() => {
                void navigator.clipboard.writeText(text)
                setCopied(true)
                setTimeout(() => setCopied(false), 1500)
              }}
              className="rounded px-2 py-0.5 text-[0.625rem] text-muted hover:bg-surface-3 hover:text-fg"
            >
              {copied ? t('preview.copied') : t('preview.copy')}
            </button>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <button
            type="button"
            onClick={() => setMode('preview')}
            className={`rounded px-2 py-0.5 text-[0.625rem] ${
              mode === 'preview'
                ? 'bg-sel font-medium text-fg-strong'
                : 'text-muted hover:bg-surface-3'
            }`}
          >
            {t('preview.htmlRender')}
          </button>
          <button
            type="button"
            onClick={() => setMode('source')}
            className={`rounded px-2 py-0.5 text-[0.625rem] ${
              mode === 'source'
                ? 'bg-sel font-medium text-fg-strong'
                : 'text-muted hover:bg-surface-3'
            }`}
          >
            {t('preview.htmlSource')}
          </button>
        </div>
      </div>
      {mode === 'preview' ? (
        <div
          className="relative grid min-h-0 flex-1 place-items-center overflow-hidden rounded border border-line p-4"
          style={{
            backgroundImage: `linear-gradient(45deg, var(--color-surface-3) 25%, transparent 25%), linear-gradient(-45deg, var(--color-surface-3) 25%, transparent 25%), linear-gradient(45deg, transparent 75%, var(--color-surface-3) 75%), linear-gradient(-45deg, transparent 75%, var(--color-surface-3) 75%)`,
            backgroundSize: '16px 16px',
            backgroundPosition: '0 0, 0 8px, 8px -8px, -8px 0px',
          }}
        >
          {url && (
            <img
              src={url}
              alt={name}
              className="max-h-full max-w-full select-none object-contain drop-shadow-sm"
            />
          )}
        </div>
      ) : (
        <pre
          data-selectable
          className="min-h-0 flex-1 overflow-auto whitespace-pre-wrap break-words font-mono text-[0.6875rem] leading-relaxed text-fg"
        >
          {text}
        </pre>
      )}
    </div>
  )
}
