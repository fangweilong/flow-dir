import { useEffect, useState } from 'react'
import { appInfo } from './api'

/** Tile size in px; the stamp is centred and rotated inside it. */
const TILE_WIDTH = 480
const TILE_HEIGHT = 160
/** Low enough to stay out of the way, high enough to survive a screenshot. */
const STAMP_OPACITY = 0.055

const pad = (value: number) => String(value).padStart(2, '0')

/** Local `YYYY-MM-DD HH:mm:ss`, independent of the UI language. */
const formatTime = (at: Date) =>
  `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())} ${pad(at.getHours())}:${pad(at.getMinutes())}:${pad(at.getSeconds())}`

const escapeXml = (value: string) =>
  value.replace(
    /[&<>"']/g,
    (char) =>
      ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&apos;' })[char] ?? char,
  )

/**
 * One tile of the background pattern. The text lives in an SVG data URI so the browser
 * repeats it for free across the whole window without a DOM node per repetition.
 */
function tileUrl(stamp: string) {
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="${TILE_WIDTH}" height="${TILE_HEIGHT}">` +
    `<text x="${TILE_WIDTH / 2}" y="${TILE_HEIGHT / 2}" text-anchor="middle" ` +
    `dominant-baseline="middle" font-family="Segoe UI, sans-serif" font-size="10" fill="#000" ` +
    `transform="rotate(-18 ${TILE_WIDTH / 2} ${TILE_HEIGHT / 2})">${escapeXml(stamp)}</text></svg>`
  return `url("data:image/svg+xml,${encodeURIComponent(svg)}")`
}

/**
 * Faint version / OS / clock overlay. Debugging aid: the stamp identifies the build and
 * environment in a screenshot, while `pointer-events-none` keeps every control clickable.
 */
export function Watermark() {
  const [version, setVersion] = useState('')
  const [osVersion, setOsVersion] = useState('')
  const [now, setNow] = useState(() => new Date())

  useEffect(() => {
    let disposed = false
    void appInfo()
      .then((info) => {
        if (disposed) return
        setVersion(info.version)
        setOsVersion(info.osVersion)
      })
      .catch(() => undefined)
    return () => {
      disposed = true
    }
  }, [])

  // The clock is the point of the stamp: a screenshot has to carry the moment it was taken.
  useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), 1000)
    return () => window.clearInterval(timer)
  }, [])

  const stamp = `${version ? `FlowDir ${version}` : 'FlowDir'} · ${osVersion || '…'} · ${formatTime(now)}`

  return (
    <div
      aria-hidden
      data-watermark
      className="pointer-events-none fixed inset-0 z-30 select-none dark:invert"
      style={{ backgroundImage: tileUrl(stamp), opacity: STAMP_OPACITY }}
    />
  )
}
