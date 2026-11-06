import { useCallback, useEffect, useRef, useState } from 'react'
import type { PointerEvent as ReactPointerEvent } from 'react'
import type { Translate } from '../../i18n'

const MIN_ZOOM = 0.1
const MAX_ZOOM = 16
const ZOOM_STEP = 1.25
const FIT: View = { zoom: 1, x: 0, y: 0 }

type View = { zoom: number; x: number; y: number }

const clamp = (value: number, min: number, max: number) => Math.min(Math.max(value, min), max)

/**
 * Image preview shared by the side panel and the fullscreen layer. The panel
 * keeps the old fit/1:1 toggle; the fullscreen layer adds cursor-anchored wheel
 * zoom and drag panning, which is the main reason the layer exists.
 */
export function ImagePreview({
  url,
  name,
  dimensions,
  expanded,
  onExpand,
  t,
}: {
  url: string
  name: string
  dimensions?: string | null
  expanded: boolean
  onExpand: () => void
  t: Translate
}) {
  const [fit, setFit] = useState(true)
  const [view, setView] = useState<View>(FIT)
  const boxRef = useRef<HTMLDivElement | null>(null)
  const imgRef = useRef<HTMLImageElement | null>(null)
  const viewRef = useRef(view)
  viewRef.current = view
  const drag = useRef<{ sx: number; sy: number; x: number; y: number } | null>(null)

  useEffect(() => {
    setView(FIT)
  }, [expanded, url])

  const zoomAt = useCallback((factor: number, anchor?: { x: number; y: number }) => {
    setView((current) => {
      const zoom = clamp(current.zoom * factor, MIN_ZOOM, MAX_ZOOM)
      const ratio = zoom / current.zoom
      // `transform-origin` is the centre: scaling the translation by the same
      // ratio keeps the zoomed point under the cursor (or the centre) fixed.
      return {
        zoom,
        x: anchor ? anchor.x - (anchor.x - current.x) * ratio : current.x * ratio,
        y: anchor ? anchor.y - (anchor.y - current.y) * ratio : current.y * ratio,
      }
    })
  }, [])

  const actualSize = useCallback(() => {
    const image = imgRef.current
    if (!image || image.clientWidth === 0) return
    setView({ zoom: clamp(image.naturalWidth / image.clientWidth, MIN_ZOOM, MAX_ZOOM), x: 0, y: 0 })
  }, [])

  // React's wheel listener is passive, so preventDefault needs a native one.
  useEffect(() => {
    if (!expanded) return
    const box = boxRef.current
    if (!box) return
    const onWheel = (event: WheelEvent) => {
      event.preventDefault()
      const rect = box.getBoundingClientRect()
      zoomAt(Math.exp(-event.deltaY * 0.0015), {
        x: event.clientX - rect.left - rect.width / 2,
        y: event.clientY - rect.top - rect.height / 2,
      })
    }
    box.addEventListener('wheel', onWheel, { passive: false })
    return () => box.removeEventListener('wheel', onWheel)
  }, [expanded, zoomAt])

  useEffect(() => {
    if (!expanded) return
    const onKey = (event: KeyboardEvent) => {
      if (event.ctrlKey || event.altKey || event.metaKey) return
      if (event.key === '+' || event.key === '=') zoomAt(ZOOM_STEP)
      else if (event.key === '-' || event.key === '_') zoomAt(1 / ZOOM_STEP)
      else if (event.key === '0') setView(FIT)
      else if (event.key === '1') actualSize()
      else return
      event.preventDefault()
      event.stopPropagation()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [actualSize, expanded, zoomAt])

  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (viewRef.current.zoom <= 1) return
    drag.current = {
      sx: event.clientX,
      sy: event.clientY,
      x: viewRef.current.x,
      y: viewRef.current.y,
    }
    event.currentTarget.setPointerCapture(event.pointerId)
  }

  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const start = drag.current
    if (!start) return
    setView((current) => ({
      ...current,
      x: start.x + event.clientX - start.sx,
      y: start.y + event.clientY - start.sy,
    }))
  }

  const endDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
    drag.current = null
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId)
    }
  }

  if (!expanded) {
    return (
      <div className="flex h-full flex-col">
        <div className="flex shrink-0 items-center justify-between pb-1 text-[0.625rem]">
          <span className="font-mono text-muted-2">{dimensions ? `${dimensions} px` : ''}</span>
          <button
            type="button"
            onClick={() => setFit((prev) => !prev)}
            className="rounded bg-surface-2 px-1.5 py-0.5 text-muted hover:bg-surface-3 hover:text-fg"
          >
            {fit ? '1:1' : 'Fit'}
          </button>
        </div>
        <div className="grid min-h-0 flex-1 place-items-center overflow-auto">
          <img
            src={url}
            alt={name}
            title={t('preview.expand')}
            onDoubleClick={onExpand}
            className={`cursor-zoom-in rounded border border-line shadow-sm ${
              fit ? 'max-h-full max-w-full object-contain' : 'max-w-none'
            }`}
          />
        </div>
      </div>
    )
  }

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between gap-2 pb-1 text-[0.625rem]">
        <span className="font-mono text-muted-2">{dimensions ? `${dimensions} px` : ''}</span>
        <div className="flex items-center gap-1">
          <button
            type="button"
            onClick={() => zoomAt(1 / ZOOM_STEP)}
            title={t('preview.zoomOut')}
            className="grid h-5 w-5 place-items-center rounded bg-surface-2 text-muted hover:bg-surface-3 hover:text-fg"
          >
            −
          </button>
          <span className="w-10 text-center font-mono text-muted-2">{Math.round(view.zoom * 100)}%</span>
          <button
            type="button"
            onClick={() => zoomAt(ZOOM_STEP)}
            title={t('preview.zoomIn')}
            className="grid h-5 w-5 place-items-center rounded bg-surface-2 text-muted hover:bg-surface-3 hover:text-fg"
          >
            +
          </button>
          <button
            type="button"
            onClick={() => setView(FIT)}
            className="rounded bg-surface-2 px-1.5 py-0.5 text-muted hover:bg-surface-3 hover:text-fg"
          >
            {t('preview.zoomFit')}
          </button>
          <button
            type="button"
            onClick={actualSize}
            className="rounded bg-surface-2 px-1.5 py-0.5 text-muted hover:bg-surface-3 hover:text-fg"
          >
            1:1
          </button>
        </div>
      </div>
      <div
        ref={boxRef}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        onDoubleClick={() => setView(FIT)}
        className={`grid min-h-0 flex-1 place-items-center overflow-hidden rounded border border-line bg-surface-2/30 ${
          view.zoom > 1 ? 'cursor-grab active:cursor-grabbing' : ''
        }`}
      >
        <img
          ref={imgRef}
          src={url}
          alt={name}
          draggable={false}
          style={{ transform: `translate(${view.x}px, ${view.y}px) scale(${view.zoom})` }}
          className="max-h-full max-w-full touch-none object-contain select-none will-change-transform"
        />
      </div>
    </div>
  )
}
