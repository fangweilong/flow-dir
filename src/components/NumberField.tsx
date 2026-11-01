import { useState } from 'react'
import type { ComponentPropsWithoutRef } from 'react'

type Props = Omit<
  ComponentPropsWithoutRef<'input'>,
  'value' | 'onChange' | 'type' | 'min' | 'max' | 'step'
> & {
  value: number
  min: number
  max: number
  step?: number
  onChange: (value: number) => void
}

/**
 * Numeric input that keeps the raw text while editing.
 *
 * A controlled `type="number"` clamped on every keystroke cannot be emptied: `''` parses
 * to 0, the clamp writes the minimum back, and the next digit appends to it ("1" + "5"
 * became 15). The draft survives only until blur, where the committed value takes over.
 */
export function NumberField({ value, min, max, step, onChange, onBlur, onKeyDown, ...rest }: Props) {
  const [draft, setDraft] = useState<string | null>(null)
  return (
    <input
      {...rest}
      type="number"
      min={min}
      max={max}
      step={step}
      value={draft ?? String(value)}
      onChange={(ev) => {
        const raw = ev.target.value
        setDraft(raw)
        // Empty (or a lone `-`) is a transient editing state; pushing a made-up value
        // into settings here is what locked the field.
        if (raw.trim() === '') return
        const next = Number(raw)
        if (!Number.isFinite(next)) return
        onChange(Math.min(max, Math.max(min, Math.round(next))))
      }}
      onBlur={(ev) => {
        setDraft(null)
        onBlur?.(ev)
      }}
      onKeyDown={(ev) => {
        if (ev.key === 'Enter') ev.currentTarget.blur()
        onKeyDown?.(ev)
      }}
    />
  )
}
