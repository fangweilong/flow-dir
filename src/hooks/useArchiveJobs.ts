import { useCallback, useState } from 'react'
import type { Dispatch, SetStateAction } from 'react'
import type { OpProgress } from '../types'
import {
  archiveCompress,
  archiveExtract,
  COMPRESS_EXTENSIONS,
  pathIsDir,
  uniquePaths,
  type CompressFormat,
} from '../api'
import { errorCode } from '../errorCode'
import type { Translate } from '../i18n'
import { errorKey } from '../i18n'
import { baseName, joinPath } from '../paths'

export interface UseArchiveJobsOptions {
  setOps: Dispatch<SetStateAction<Record<string, OpProgress>>>
  refreshPanes: () => void
  showToast: (msg: string, timeout?: number) => void
  t: Translate
}

/**
 * Extraction and compression jobs, kept separate from `useFileOperations` because they
 * have their own progress records, password prompt and target dialogs.
 */
export function useArchiveJobs({ setOps, refreshPanes, showToast, t }: UseArchiveJobsOptions) {
  const [passwordPrompt, setPasswordPrompt] = useState<{
    archive: string
    dest: string
    wrong: boolean
  } | null>(null)
  const [extractTarget, setExtractTarget] = useState<{ archive: string; initial: string } | null>(
    null,
  )
  const [compressTarget, setCompressTarget] = useState<{
    paths: string[]
    dir: string
    format: CompressFormat
    name: string
  } | null>(null)

  const extractArchive = useCallback(
    (archive: string, dest: string, password?: string) => {
      const opId = `extract-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
      setOps((prev) => ({
        ...prev,
        [opId]: {
          opId,
          phase: 'extract',
          current: 0,
          total: 0,
          name: null,
          work: 0,
          workTotal: 0,
          failedCount: 0,
          failed: [],
          done: false,
          aborted: false,
          error: null,
        },
      }))
      const drop = (delay: number) =>
        window.setTimeout(() => {
          setOps((prev) => {
            const next = { ...prev }
            delete next[opId]
            return next
          })
        }, delay)
      archiveExtract(opId, archive, dest, password ?? null, (progress) => {
        setOps((prev) => (prev[opId] ? { ...prev, [opId]: progress } : prev))
        if (!progress.done) return
        if (progress.error) {
          const code = progress.error.code
          if (code === 'PASSWORD_REQUIRED' || code === 'WRONG_PASSWORD') {
            setPasswordPrompt({ archive, dest, wrong: code === 'WRONG_PASSWORD' })
            drop(3000)
            return
          }
          showToast(t('op.failed', { code: t(errorKey(code)) }))
          drop(5000)
          return
        }
        if (progress.aborted) {
          showToast(t('op.canceled'))
          drop(2500)
          return
        }
        refreshPanes()
        if (progress.failedCount > 0) {
          showToast(
            t('op.partial', {
              ok: Math.max(0, progress.total - progress.failedCount),
              failed: progress.failedCount,
            }),
            8000,
          )
          drop(30000)
          return
        }
        showToast(t('op.done'))
        drop(800)
      }).catch((e: unknown) => {
        showToast(t('op.failed', { code: t(errorKey(errorCode(e))) }))
        drop(4000)
      })
    },
    [refreshPanes, setOps, showToast, t],
  )

  const runCompress = useCallback(
    (paths: string[], dest: string, format: CompressFormat, level: number, password: string | null) => {
      const opId = `compress-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
      setOps((prev) => ({
        ...prev,
        [opId]: {
          opId,
          phase: 'compress',
          current: 0,
          total: 0,
          name: null,
          work: 0,
          workTotal: 0,
          failedCount: 0,
          failed: [],
          done: false,
          aborted: false,
          error: null,
        },
      }))
      const drop = (delay: number) =>
        window.setTimeout(() => {
          setOps((prev) => {
            const next = { ...prev }
            delete next[opId]
            return next
          })
        }, delay)
      archiveCompress(opId, paths, dest, format, level, password, (progress) => {
        setOps((prev) => (prev[opId] ? { ...prev, [opId]: progress } : prev))
        if (!progress.done) return
        if (progress.error) {
          showToast(t('op.failed', { code: t(errorKey(progress.error.code)) }))
          drop(5000)
          return
        }
        if (progress.aborted) {
          showToast(t('op.canceled'))
          drop(2500)
          return
        }
        refreshPanes()
        if (progress.failedCount > 0) {
          // Unreadable entries are skipped and still produce an archive; reporting plain
          // "done" would hide that files are missing from it.
          showToast(
            t('op.partial', {
              ok: Math.max(0, progress.total - progress.failedCount),
              failed: progress.failedCount,
            }),
          )
          drop(15000)
          return
        }
        showToast(t('op.done'))
        drop(800)
      }).catch((e: unknown) => {
        showToast(t('op.failed', { code: t(errorKey(errorCode(e))) }))
        drop(4000)
      })
    },
    [refreshPanes, setOps, showToast, t],
  )

  const requestCompress = useCallback(
    (paths: string[], dir: string, format: CompressFormat) => {
      const extension = COMPRESS_EXTENSIONS[format]
      void (async () => {
        let base = baseName(dir) || 'archive'
        if (paths.length === 1) {
          const full = baseName(paths[0])
          // Files compress to `report.zip`, not `report.docx.zip`; a folder keeps its full
          // name because dots are legitimate there.
          const dot = full.lastIndexOf('.')
          base =
            dot > 0 && dot < full.length - 1 && !(await pathIsDir(paths[0]))
              ? full.slice(0, dot)
              : full
        }
        const [dest] = await uniquePaths([joinPath(dir, `${base}${extension}`)])
        setCompressTarget({ paths, dir, format, name: baseName(dest) })
      })().catch((e: unknown) =>
        showToast(t('op.failed', { code: t(errorKey(errorCode(e))) })),
      )
    },
    [showToast, t],
  )

  return {
    passwordPrompt,
    setPasswordPrompt,
    extractTarget,
    setExtractTarget,
    compressTarget,
    setCompressTarget,
    extractArchive,
    runCompress,
    requestCompress,
  }
}
