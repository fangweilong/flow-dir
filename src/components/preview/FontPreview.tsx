import { useEffect, useMemo, useState } from 'react'
import type { Translate } from '../../i18n'

type Props = {
  url: string
  name: string
  mtime: number
  t: Translate
}

export function FontPreview({ url, name, mtime, t }: Props) {
  const fontFamily = useMemo(
    () => `FontPreview_${mtime}_${name.replace(/\W/g, '_')}`,
    [mtime, name]
  )
  const [customText, setCustomText] = useState('')

  useEffect(() => {
    const style = document.createElement('style')
    style.id = `font-face-${fontFamily}`
    style.textContent = `
      @font-face {
        font-family: '${fontFamily}';
        src: url('${url}');
        font-display: swap;
      }
    `
    document.head.appendChild(style)
    return () => {
      style.remove()
    }
  }, [fontFamily, url])

  const displayText = customText.trim() || 'The quick brown fox jumps over the lazy dog'

  return (
    <div className="flex h-full flex-col gap-3 overflow-auto pr-1">
      <div className="shrink-0 space-y-1">
        <input
          type="text"
          value={customText}
          onChange={(e) => setCustomText(e.target.value)}
          placeholder={t('preview.fontCustom')}
          className="h-7 w-full rounded border border-line bg-surface-2 px-2 text-[0.6875rem] text-fg outline-none placeholder:text-muted-2 focus:border-blue-400"
        />
      </div>

      <div className="space-y-3 font-normal" style={{ fontFamily }}>
        <div className="rounded border border-line bg-surface-2/40 p-3">
          <div className="text-[2rem] leading-tight text-fg-strong break-words">
            {displayText}
          </div>
        </div>

        <div className="space-y-2 rounded border border-line p-3">
          <div className="text-[0.625rem] font-semibold tracking-wider text-muted-2 font-sans">
            {t('preview.fontSpecimen')}
          </div>
          <div className="text-[1.25rem] leading-snug text-fg-strong break-words">
            天地玄黄 宇宙洪荒 日月盈昃 辰宿列张
          </div>
          <div className="text-[1rem] leading-normal text-fg break-words">
            FlowDir · 0123456789 · !@#$%^&*()_+-=~
          </div>
          <div className="text-[0.8125rem] leading-normal text-muted break-words">
            Sphinx of black quartz, judge my vow.
          </div>
        </div>

        <div className="space-y-1.5 rounded border border-line p-3">
          <div className="text-[0.625rem] font-semibold tracking-wider text-muted-2 font-sans">
            {t('preview.fontAlphabet')}
          </div>
          <div className="text-[0.8125rem] leading-relaxed text-fg break-all tracking-wider">
            A B C D E F G H I J K L M N O P Q R S T U V W X Y Z
          </div>
          <div className="text-[0.8125rem] leading-relaxed text-fg break-all tracking-wider">
            a b c d e f g h i j k l m n o p q r s t u v w x y z
          </div>
          <div className="text-[0.8125rem] leading-relaxed text-muted break-all tracking-wider">
            0 1 2 3 4 5 6 7 8 9 ` ~ ! @ # $ % ^ & * ( ) _ + - = [ ] &#123; &#125; | ; : &apos; &quot; , . / &lt; &gt; ?
          </div>
        </div>
      </div>
    </div>
  )
}
