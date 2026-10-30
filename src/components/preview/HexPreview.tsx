import { useState } from 'react'
import type { Translate } from '../../i18n'

type Props = {
  hex: string | null | undefined
  t: Translate
}

export function HexPreview({ hex, t }: Props) {
  const [copied, setCopied] = useState(false)

  const copyHex = () => {
    if (!hex) return
    void navigator.clipboard.writeText(hex)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between pb-1.5">
        <span className="text-[0.625rem] text-muted-2 font-mono">
          {t('preview.hex')}
        </span>
        <button
          type="button"
          onClick={copyHex}
          className="rounded px-2 py-0.5 text-[0.625rem] text-muted hover:bg-surface-3 hover:text-fg"
        >
          {copied ? t('preview.copied') : t('preview.copy')}
        </button>
      </div>

      <pre
        data-selectable
        className="min-h-0 flex-1 overflow-auto whitespace-pre rounded border border-line bg-surface-2/40 p-2 font-mono text-[0.65625rem] leading-tight text-fg selection:bg-sel"
      >
        {hex}
      </pre>
    </div>
  )
}
