import type { Key } from '../../i18n'
import type { LogLevel } from '../../persist'
import type { Theme } from '../../types'
import { themeIsDark } from '../../themes'
import { FONT_SIZE_MAX, REGION_FONT_SIZE_MIN } from '../../fonts'

export type SectionId =
  | 'general'
  | 'columns'
  | 'shortcuts'
  | 'integration'
  | 'graphics'
  | 'quickAccess'
  | 'preview'
  | 'config'
  | 'updates'
  | 'diagnostics'
  | 'about'

export const SECTIONS: { id: SectionId; label: Key }[] = [
  { id: 'general', label: 'settings.general' },
  { id: 'columns', label: 'settings.columns' },
  { id: 'shortcuts', label: 'settings.shortcuts' },
  { id: 'integration', label: 'settings.integration' },
  { id: 'graphics', label: 'settings.graphics' },
  { id: 'quickAccess', label: 'settings.quickAccess' },
  { id: 'preview', label: 'settings.previewSection' },
  { id: 'config', label: 'settings.configDir' },
  { id: 'updates', label: 'settings.updates' },
  { id: 'diagnostics', label: 'settings.diagnostics' },
  { id: 'about', label: 'settings.about' },
]

export const formatBytes = (bytes: number) => {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB']
  let value = bytes
  let unitIndex = 0
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024
    unitIndex += 1
  }
  return `${value >= 100 || unitIndex === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[unitIndex]}`
}

export const UPDATE_INTERVALS = [0, 6, 12, 24, 72]
export const LOG_LEVELS: LogLevel[] = ['error', 'warn', 'info', 'debug']
/** Keys the native `RegisterHotKey` binding can map; keep in step with `shell_integration`. */
export const GLOBAL_HOTKEY_KEY =
  /^(f([1-9]|1[0-9]|2[0-4])|[a-z0-9]|space|tab|enter|esc|delete|backspace|up|down|left|right|comma)$/

/** IPC failures arrive as the serialized ApiError, so `String(err)` would say `[object Object]`. */
export const ipcErrorText = (err: unknown) => {
  if (err && typeof err === 'object' && 'code' in err) {
    const { code, detail } = err as { code: string; detail?: string }
    return detail ? `${code}: ${detail}` : code
  }
  return String(err)
}

export function ToggleRow({
  label,
  hint,
  value,
  onChange,
}: {
  label: string
  hint?: string
  value: boolean
  onChange: (value: boolean) => void
}) {
  return (
    <div className="flex items-center justify-between rounded-lg border border-line px-3 py-2">
      <div className="min-w-0">
        <div className="text-[0.75rem] text-fg">{label}</div>
        {hint && <div className="truncate text-[0.65625rem] text-muted-2">{hint}</div>}
      </div>
      <button
        type="button"
        role="switch"
        aria-checked={value}
        data-toggle={label}
        onClick={() => onChange(!value)}
        className={`relative h-5 w-9 shrink-0 rounded-full transition-colors ${value ? 'bg-blue-600' : 'bg-surface-3'}`}
      >
        <span
          className={`absolute top-0.5 h-4 w-4 rounded-full bg-white shadow transition-[left] ${value ? 'left-[1.125rem]' : 'left-0.5'}`}
        />
      </button>
    </div>
  )
}

export function ThemeSwatchSurface({ theme }: { theme: Exclude<Theme, 'system'> }) {
  const dark = themeIsDark(theme, false)
  return (
    <div data-theme={theme} className={`h-full ${dark ? 'dark' : ''}`}>
      <div className="flex h-full flex-col bg-surface">
        <div className="flex h-3 shrink-0 items-center gap-0.5 border-b border-line bg-surface-2 px-1.5">
          <span className="h-1 w-1 rounded-full bg-muted-2" />
          <span className="h-1 w-1 rounded-full bg-muted-2" />
          <span className="ml-auto h-1 w-5 rounded-full bg-surface-3" />
        </div>
        <div className="min-h-0 flex-1 px-1.5 py-1">
          <div className="flex items-center gap-1 border-b border-row-line py-[0.125rem]">
            <span className="h-1.5 w-1.5 rounded-sm bg-blue-500" />
            <span className="h-1 w-7 rounded bg-fg-strong/80" />
          </div>
          <div className="flex items-center gap-1 py-[0.125rem]">
            <span className="h-1.5 w-1.5 rounded-sm bg-muted-2" />
            <span className="h-1 w-5 rounded bg-muted-2" />
          </div>
        </div>
      </div>
    </div>
  )
}

export function ThemeSwatch({ theme }: { theme: Theme }) {
  if (theme === 'system') {
    return (
      <div className="grid h-[3.5rem] grid-cols-2 gap-px overflow-hidden rounded-md border border-line bg-line">
        <ThemeSwatchSurface theme="light" />
        <ThemeSwatchSurface theme="dark" />
      </div>
    )
  }
  return (
    <div className="h-[3.5rem] overflow-hidden rounded-md border border-line">
      <ThemeSwatchSurface theme={theme} />
    </div>
  )
}

export function ActiveCheck() {
  return (
    <svg aria-hidden viewBox="0 0 16 16" className="ml-auto h-3 w-3 shrink-0 text-blue-500">
      <path
        d="m3.5 8.5 3 3 6-7"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.75"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  )
}

export function ThemeCard({
  theme,
  label,
  active,
  onSelect,
}: {
  theme: Theme
  label: string
  active: boolean
  onSelect: () => void
}) {
  return (
    <button
      type="button"
      aria-pressed={active}
      title={label}
      onClick={onSelect}
      className={`rounded-lg border p-1.5 text-left focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue-400 ${
        active ? 'border-blue-400 bg-surface-3/60' : 'border-line hover:border-line-strong hover:bg-surface-2'
      }`}
    >
      <ThemeSwatch theme={theme} />
      <span className="mt-1.5 flex items-center gap-1 px-0.5">
        <span className={`truncate text-[0.6875rem] ${active ? 'font-medium text-fg-strong' : 'text-muted'}`}>
          {label}
        </span>
        {active && <ActiveCheck />}
      </span>
    </button>
  )
}

export function RegionFontRow({
  label,
  family,
  size,
  inheritHint,
  onChangeFamily,
  onChangeSize,
}: {
  label: string
  family: string
  size: number
  inheritHint: string
  onChangeFamily: (family: string) => void
  onChangeSize: (size: number) => void
}) {
  return (
    <div className="flex items-center gap-2">
      <span className="w-14 shrink-0 text-[0.71875rem] text-fg">{label}</span>
      <input
        list="font-family-options"
        value={family}
        placeholder={inheritHint}
        aria-label={label}
        onChange={(ev) => onChangeFamily(ev.target.value)}
        className="h-6 min-w-0 flex-1 rounded border border-line bg-surface-2 px-1.5 text-[0.6875rem] outline-none placeholder:text-muted-2 focus:border-blue-400 focus:bg-surface"
      />
      <input
        type="range"
        min={REGION_FONT_SIZE_MIN}
        max={FONT_SIZE_MAX}
        step={1}
        value={size}
        aria-label={label}
        onChange={(ev) => onChangeSize(Number(ev.target.value))}
        className="h-4 w-[5rem] shrink-0 cursor-pointer accent-blue-600"
      />
      <span className="tabular w-9 shrink-0 text-right text-[0.71875rem] text-muted">{size} px</span>
    </div>
  )
}
