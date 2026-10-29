import React, { useState } from 'react'
import { DEFAULT_SETTINGS } from '../persist'

export type ColKey = 'size' | 'type' | 'time' | 'capacity' | 'free'

export const MIN_COL_SIZE = 56
export const MIN_COL_TIME = 120
export const MIN_DRIVE_CAPACITY = 120
export const MIN_DRIVE_FREE = 56
export const DRIVE_TYPE_COL = 72
export const MIN_COL_TYPE = 56
export const MAX_COL_WIDTH = 480
export const MIN_COL_NAME = 140
export const MIN_WIDTH_FILTER = 420

export interface UsePaneColumnResizeOptions {
  width: number
  listScale: number
  showColSize: boolean
  showColType: boolean
  showColTime: boolean
  colSize: number
  colType: number
  colTime: number
  colDriveCapacity: number
  colDriveFree: number
  listNameWidth: number
  headerRef: React.RefObject<HTMLDivElement | null>
  scrollRef: React.RefObject<HTMLDivElement | null>
  onFocusPane: () => void
  onColWidths: (size: number, time: number, type: number, capacity: number, free: number) => void
  onListNameWidth: (width: number) => void
}

export function usePaneColumnResize({
  width,
  listScale,
  showColSize,
  showColType,
  showColTime,
  colSize,
  colType,
  colTime,
  colDriveCapacity,
  colDriveFree,
  listNameWidth,
  headerRef,
  scrollRef,
  onFocusPane,
  onColWidths,
  onListNameWidth,
}: UsePaneColumnResizeOptions) {
  const [draft, setDraft] = useState<{
    size: number
    type: number
    time: number
    capacity: number
    free: number
  } | null>(null)
  const [nameDraft, setNameDraft] = useState<number | null>(null)

  const nameMin = Math.round(MIN_COL_NAME * listScale)
  const minSizeCol = Math.round(MIN_COL_SIZE * listScale)
  const minTypeCol = Math.round(MIN_COL_TYPE * listScale)
  const minTimeCol = Math.round(MIN_COL_TIME * listScale)
  const wantSize = Math.round((draft?.size ?? colSize) * listScale)
  const wantType = Math.round((draft?.type ?? colType) * listScale)
  const wantTime = Math.round((draft?.time ?? colTime) * listScale)

  const showSize = showColSize && (width === 0 || width >= nameMin + minSizeCol)
  const showType = showColType && (width === 0 || width >= nameMin + (showSize ? minSizeCol : 0) + minTypeCol)
  const showTime =
    showColTime &&
    (width === 0 ||
      width >= nameMin + (showSize ? minSizeCol : 0) + (showType ? minTypeCol : 0) + minTimeCol)

  const sizeCol = Math.max(minSizeCol, wantSize)
  const typeCol = Math.max(minTypeCol, wantType)
  const timeCol = Math.max(minTimeCol, wantTime)
  const columnsWidth = (showSize ? sizeCol : 0) + (showType ? typeCol : 0) + (showTime ? timeCol : 0)

  const nameWidth =
    nameDraft !== null
      ? Math.max(nameMin, Math.round(nameDraft))
      : listNameWidth > 0
        ? Math.max(nameMin, Math.round(listNameWidth * listScale))
        : 0
  const listWidth = (nameWidth || nameMin) + columnsWidth
  const listMinWidth = `calc(${listWidth}px + 0.375rem)`
  const listSpan = nameWidth > 0 ? listMinWidth : undefined
  const showFilter = width === 0 || width >= Math.round(MIN_WIDTH_FILTER * listScale)
  const showDriveLabel = width === 0 || width >= 300
  const driveTypeCol = Math.round(DRIVE_TYPE_COL * listScale)
  const minDriveCapacity = Math.round(MIN_DRIVE_CAPACITY * listScale)
  const minDriveFree = Math.round(MIN_DRIVE_FREE * listScale)
  const wantCapacity = Math.round((draft?.capacity ?? colDriveCapacity) * listScale)
  const wantFree = Math.round((draft?.free ?? colDriveFree) * listScale)
  const showDriveCapacity =
    width === 0 || width >= nameMin + driveTypeCol + minDriveCapacity + minDriveFree
  const capacityCol =
    width === 0
      ? wantCapacity
      : Math.min(
          wantCapacity,
          Math.max(minDriveCapacity, width - nameMin - driveTypeCol - (showDriveCapacity ? minDriveFree : 0)),
        )
  const freeCol =
    width === 0
      ? wantFree
      : Math.min(wantFree, Math.max(minDriveFree, width - nameMin - driveTypeCol - capacityCol))

  const driveGrid = [
    'minmax(0,2fr)',
    ...(showDriveLabel ? ['minmax(0,1fr)'] : []),
    `${driveTypeCol}px`,
    ...(showDriveCapacity ? [`${capacityCol}px`, `${freeCol}px`] : []),
  ].join(' ')

  const grid = [
    nameWidth > 0 ? `${nameWidth}px` : `minmax(${nameMin}px, 1fr)`,
    showSize && `${sizeCol}px`,
    showType && `${typeCol}px`,
    showTime && `${timeCol}px`,
  ]
    .filter(Boolean)
    .join(' ')

  const startColDrag = (key: ColKey, ev: React.PointerEvent<HTMLSpanElement>) => {
    ev.preventDefault()
    ev.stopPropagation()
    onFocusPane()
    const startX = ev.clientX
    const rendered: Record<ColKey, number> = {
      size: sizeCol,
      type: typeCol,
      time: timeCol,
      capacity: capacityCol,
      free: freeCol,
    }
    const stored: Record<ColKey, number> = {
      size: colSize,
      type: colType,
      time: colTime,
      capacity: colDriveCapacity,
      free: colDriveFree,
    }
    const floors: Record<ColKey, number> = {
      size: MIN_COL_SIZE,
      type: MIN_COL_TYPE,
      time: MIN_COL_TIME,
      capacity: MIN_DRIVE_CAPACITY,
      free: MIN_DRIVE_FREE,
    }
    const fixed =
      key === 'capacity' ? driveTypeCol + freeCol : key === 'free' ? driveTypeCol + capacityCol : 0
    const start = rendered[key] / listScale
    const min = floors[key]
    const max =
      key === 'size' || key === 'type' || key === 'time' || width === 0
        ? MAX_COL_WIDTH
        : Math.max(min, Math.min(MAX_COL_WIDTH, (width - nameMin - fixed) / listScale))
    let next = start
    let moved = false
    const live = (): { size: number; type: number; time: number; capacity: number; free: number } => ({
      size: stored.size,
      type: stored.type,
      time: stored.time,
      capacity: stored.capacity,
      free: stored.free,
      [key]: next,
    })
    const move = (e: PointerEvent) => {
      next = Math.min(max, Math.max(min, start - (e.clientX - startX) / listScale))
      const delta = Math.abs(e.clientX - startX)
      if (delta > 1) moved = true
      setDraft(live())
    }
    const end = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', end)
      window.removeEventListener('pointercancel', end)
      setDraft(null)
      if (!moved || Math.round(next) === Math.round(start)) return
      const widths = live()
      onColWidths(
        Math.round(widths.size),
        Math.round(widths.type),
        Math.round(widths.time),
        Math.round(widths.capacity),
        Math.round(widths.free),
      )
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', end)
    window.addEventListener('pointercancel', end)
  }

  const resetCol = (key: ColKey) => {
    const defaults: Record<ColKey, number> = {
      size: DEFAULT_SETTINGS.listSizeWidth,
      type: DEFAULT_SETTINGS.listTypeWidth,
      time: DEFAULT_SETTINGS.listTimeWidth,
      capacity: DEFAULT_SETTINGS.driveCapacityWidth,
      free: DEFAULT_SETTINGS.driveFreeWidth,
    }
    const stored: Record<ColKey, number> = {
      size: colSize,
      type: colType,
      time: colTime,
      capacity: colDriveCapacity,
      free: colDriveFree,
    }
    const widths = { ...stored, [key]: defaults[key] }
    onColWidths(
      Math.round(widths.size),
      Math.round(widths.type),
      Math.round(widths.time),
      Math.round(widths.capacity),
      Math.round(widths.free),
    )
  }

  const startNameDrag = (ev: React.PointerEvent<HTMLSpanElement>) => {
    ev.preventDefault()
    ev.stopPropagation()
    onFocusPane()
    const startX = ev.clientX
    const head = headerRef.current?.firstElementChild as HTMLElement | null
    const startName = nameWidth || Math.max(nameMin, Math.round(head?.getBoundingClientRect().width ?? nameMin))
    const paneWidth = scrollRef.current?.clientWidth ?? width
    const maxName = Math.max(startName, paneWidth - columnsWidth - 6)
    let next = startName
    let moved = false
    const move = (e: PointerEvent) => {
      next = Math.min(maxName, Math.max(nameMin, startName + (e.clientX - startX)))
      if (Math.abs(e.clientX - startX) > 1) moved = true
      setNameDraft(next)
    }
    const end = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', end)
      window.removeEventListener('pointercancel', end)
      setNameDraft(null)
      if (!moved) return
      const filled = next >= maxName - 1
      onListNameWidth(filled ? 0 : Math.round(next / listScale))
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', end)
    window.addEventListener('pointercancel', end)
  }

  return {
    draft,
    nameDraft,
    nameMin,
    nameWidth,
    sizeCol,
    typeCol,
    timeCol,
    capacityCol,
    freeCol,
    columnsWidth,
    listMinWidth,
    listSpan,
    showSize,
    showType,
    showTime,
    showFilter,
    showDriveLabel,
    showDriveCapacity,
    driveTypeCol,
    driveGrid,
    grid,
    startColDrag,
    resetCol,
    startNameDrag,
  }
}
