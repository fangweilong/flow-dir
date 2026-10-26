import { useState } from 'react'

const EXT_GROUP: Record<string, string> = {
  jpg: 'image', jpeg: 'image', png: 'image', gif: 'image', bmp: 'image', webp: 'image',
  svg: 'image', ico: 'image', tiff: 'image', heic: 'image',
  mp4: 'video', mkv: 'video', mov: 'video', avi: 'video', webm: 'video', flv: 'video',
  mp3: 'audio', flac: 'audio', wav: 'audio', m4a: 'audio', ogg: 'audio',
  zip: 'archive', rar: 'archive', '7z': 'archive', tar: 'archive', gz: 'archive', xz: 'archive', iso: 'archive',
  pdf: 'pdf',
  doc: 'doc', docx: 'doc', xls: 'doc', xlsx: 'doc', ppt: 'doc', pptx: 'doc', txt: 'doc', md: 'doc', rtf: 'doc',
  ts: 'code', tsx: 'code', js: 'code', jsx: 'code', json: 'code', rs: 'code', py: 'code',
  java: 'code', c: 'code', h: 'code', cpp: 'code', cs: 'code', go: 'code', html: 'code', css: 'code',
  exe: 'exe', dll: 'exe', msi: 'exe', bat: 'exe', cmd: 'exe', ps1: 'exe',
}

const COLOR: Record<string, string> = {
  image: 'text-emerald-500',
  video: 'text-violet-500',
  audio: 'text-pink-500',
  archive: 'text-orange-500',
  pdf: 'text-red-500',
  doc: 'text-sky-500',
  code: 'text-cyan-600',
  exe: 'text-indigo-500',
  folder: 'text-amber-400',
  file: 'text-slate-400',
}

function extOf(name: string) {
  const i = name.lastIndexOf('.')
  return i > 0 ? name.slice(i + 1).toLowerCase() : ''
}

export function FileIcon({
  name,
  dir,
  path,
  scale = 1,
}: {
  name: string
  dir: boolean
  path?: string
  /** Base-text multiplier used only for the requested raster size; CSS keeps the icon at 1rem. */
  scale?: number
}) {
  const [failed, setFailed] = useState(false)
  if (path && !failed) {
    const px = Math.max(16, Math.round(16 * scale * (window.devicePixelRatio || 1)))
    return (
      <img
        src={`http://fd.localhost/icon?path=${encodeURIComponent(path)}&s=${px}`}
        width={16}
        height={16}
        alt=""
        draggable={false}
        className="shrink-0"
        style={{ width: '1rem', height: '1rem' }}
        onError={() => setFailed(true)}
      />
    )
  }
  if (dir) {
    return (
      <svg viewBox="0 0 16 16" className={`h-4 w-4 shrink-0 ${COLOR.folder}`} aria-hidden>
        <path
          fill="currentColor"
          d="M1.5 3.6c0-.6.5-1.1 1.1-1.1h3.2c.4 0 .7.15.95.42l.85.93h6.3c.6 0 1.1.5 1.1 1.1v7.4c0 .6-.5 1.1-1.1 1.1H2.6c-.6 0-1.1-.5-1.1-1.1V3.6Z"
        />
      </svg>
    )
  }
  const group = EXT_GROUP[extOf(name)] ?? 'file'
  return (
    <svg viewBox="0 0 16 16" className={`h-4 w-4 shrink-0 ${COLOR[group] ?? COLOR.file}`} aria-hidden>
      <path
        fill="currentColor"
        d="M3.2 1.5h6.05L13 5.25v9.25c0 .55-.45 1-1 1h-8.8c-.55 0-1-.45-1-1v-12c0-.55.45-1 1-1Z"
        opacity="0.95"
      />
      <path fill="#fff" opacity="0.9" d="M9.25 1.6 13 5.3H9.75c-.28 0-.5-.22-.5-.5V1.6Z" />
      <path stroke="#fff" strokeLinecap="round" strokeWidth="1.1" opacity="0.85" d="M5 9h6M5 11.2h4.2" />
    </svg>
  )
}
