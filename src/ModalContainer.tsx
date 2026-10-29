import { lazy, Suspense } from 'react'
import type { RefObject } from 'react'
import type { Lang, OpKind } from './types'
import type { Settings } from './persist'
import type { PaneState } from './workspace'
import type { Translate } from './i18n'
import type { RunOpOptions } from './hooks/useFileOperations'
import type { UndoEntry } from './undo'
import type { SyncAction } from './ComparePanel'
import { ContextMenu } from './ContextMenu'
import type { MenuItem } from './ContextMenu'
import { PromptDialog } from './PromptDialog'
import { ConfirmDialog } from './ConfirmDialog'
import type { CompressValues } from './CompressDialog'
import { baseName, joinPath } from './paths'
import { COMPRESS_EXTENSIONS, uniquePaths, type CompressFormat } from './api'
import { errorCode } from './hooks/useFileOperations'

const FolderPickerDialog = lazy(() =>
  import('./FolderPickerDialog').then((m) => ({ default: m.FolderPickerDialog })),
)
const HistoryPanel = lazy(() =>
  import('./HistoryPanel').then((m) => ({ default: m.HistoryPanel })),
)
const BatchRenameDialog = lazy(() =>
  import('./BatchRenameDialog').then((m) => ({ default: m.BatchRenameDialog })),
)
const CompressDialog = lazy(() =>
  import('./CompressDialog').then((m) => ({ default: m.CompressDialog })),
)
const FileToolsPanel = lazy(() =>
  import('./FileToolsPanel').then((m) => ({ default: m.FileToolsPanel })),
)
const ComparePanel = lazy(() =>
  import('./ComparePanel').then((m) => ({ default: m.ComparePanel })),
)
const SettingsModal = lazy(() =>
  import('./SettingsModal').then((m) => ({ default: m.SettingsModal })),
)

export type PromptState = {
  title: string
  initial: string
  paneId: string
  submit: (value: string) => void
}

export type DeleteConfirmState = {
  paneId: string
  names: string[]
  permanent: boolean
}

export type BatchRenameState = {
  dir: string
  paneId: string
  names: string[]
}

export type FileToolsState =
  | { mode: 'hash'; paths: string[] }
  | { mode: 'dup'; root: string }
export interface ModalContainerProps {
  lang: Lang
  t: Translate
  settings: Settings
  updateSettings: (next: Settings) => void
  activePane: PaneState | undefined
  focusList: (paneId: string) => void
  showToast: (msg: string) => void
  runOp: (kind: OpKind, sources: string[], options?: RunOpOptions) => void
  extractArchive: (archive: string, dest: string, password?: string) => void
  runCompress: (paths: string[], dest: string, format: CompressFormat, level: number, password: string | null) => void
  runSyncActions: (actions: SyncAction[]) => Promise<void>
  openInApp: (path: string) => void
  pathsFor: (paneId: string, names: string[]) => string[]
  openFolders: string[]
  comparePaths: string[]

  historyOpen: boolean
  setHistoryOpen: (open: boolean) => void
  historyEpoch: number
  undoStack: RefObject<UndoEntry[]>
  redoStack: RefObject<UndoEntry[]>
  undoTo: (id: number) => void
  redoLast: () => void

  menu: { x: number; y: number; items: MenuItem[] } | null
  setMenu: (menu: { x: number; y: number; items: MenuItem[] } | null) => void

  passwordPrompt: { archive: string; dest: string; wrong: boolean } | null
  setPasswordPrompt: (p: { archive: string; dest: string; wrong: boolean } | null) => void

  extractTarget: { archive: string; initial: string } | null
  setExtractTarget: (t: { archive: string; initial: string } | null) => void

  prompt: PromptState | null
  setPrompt: (p: PromptState | null) => void

  compressTarget: { paths: string[]; dir: string; format: CompressFormat; name: string } | null
  setCompressTarget: (t: { paths: string[]; dir: string; format: CompressFormat; name: string } | null) => void

  batchRename: BatchRenameState | null
  setBatchRename: (b: BatchRenameState | null) => void

  fileTools: FileToolsState | null
  setFileTools: (f: FileToolsState | null) => void

  compareOpen: boolean
  setCompareOpen: (open: boolean) => void

  deleteConfirm: DeleteConfirmState | null
  setDeleteConfirm: (d: DeleteConfirmState | null) => void
  confirmDetail: string

  quitBusy: number | null
  setQuitBusy: (q: number | null) => void
  quitApp: () => Promise<void>

  settingsOpen: boolean
  setSettingsOpen: (open: boolean) => void
}

export function ModalContainer({
  lang,
  t,
  settings,
  updateSettings,
  activePane,
  focusList,
  showToast,
  runOp,
  extractArchive,
  runCompress,
  runSyncActions,
  openInApp,
  pathsFor,
  openFolders,
  comparePaths,

  historyOpen,
  setHistoryOpen,
  historyEpoch,
  undoStack,
  redoStack,
  undoTo,
  redoLast,

  menu,
  setMenu,

  passwordPrompt,
  setPasswordPrompt,

  extractTarget,
  setExtractTarget,

  prompt,
  setPrompt,

  compressTarget,
  setCompressTarget,

  batchRename,
  setBatchRename,

  fileTools,
  setFileTools,

  compareOpen,
  setCompareOpen,

  deleteConfirm,
  setDeleteConfirm,
  confirmDetail,

  quitBusy,
  setQuitBusy,
  quitApp,

  settingsOpen,
  setSettingsOpen,
}: ModalContainerProps) {
  return (
    <>
      <Suspense fallback={null}>
        {historyOpen && (
          <HistoryPanel
            key={historyEpoch}
            lang={lang}
            undo={undoStack.current ?? []}
            redo={redoStack.current ?? []}
            onUndoTo={undoTo}
            onRedo={redoLast}
            onClose={() => setHistoryOpen(false)}
          />
        )}
      </Suspense>

      {menu && <ContextMenu x={menu.x} y={menu.y} items={menu.items} onClose={() => setMenu(null)} />}

      {passwordPrompt && (
        <PromptDialog
          title={t('archive.passwordTitle')}
          initial=""
          password
          hint={
            passwordPrompt.wrong
              ? t('archive.passwordWrong')
              : t('archive.passwordHint', { name: baseName(passwordPrompt.archive) })
          }
          lang={lang}
          onConfirm={(value) => {
            const pending = passwordPrompt
            setPasswordPrompt(null)
            extractArchive(pending.archive, pending.dest, value)
          }}
          onClose={() => setPasswordPrompt(null)}
        />
      )}

      <Suspense fallback={null}>
        {extractTarget && (
          <FolderPickerDialog
            title={t('menu.extractTo')}
            initial={extractTarget.initial}
            lang={lang}
            onPick={(target) => {
              const pending = extractTarget
              setExtractTarget(null)
              extractArchive(pending.archive, target)
            }}
            onClose={() => setExtractTarget(null)}
          />
        )}
      </Suspense>

      {prompt && (
        <PromptDialog
          title={prompt.title}
          initial={prompt.initial}
          lang={lang}
          onConfirm={(value) => {
            const pending = prompt
            prompt.submit(value)
            setPrompt(null)
            focusList(pending.paneId)
          }}
          onClose={() => {
            const pending = prompt
            setPrompt(null)
            if (pending) focusList(pending.paneId)
          }}
        />
      )}

      <Suspense fallback={null}>
        {compressTarget && (
          <CompressDialog
            title={t('compress.title')}
            dir={compressTarget.dir}
            name={compressTarget.name}
            format={compressTarget.format}
            lang={lang}
            onClose={() => setCompressTarget(null)}
            onSubmit={(values: CompressValues) => {
              const pending = compressTarget
              setCompressTarget(null)
              const extension = COMPRESS_EXTENSIONS[values.format]
              const name = values.name.toLowerCase().endsWith(extension)
                ? values.name
                : `${values.name}${extension}`
              void uniquePaths([joinPath(pending.dir, name)])
                .then(([dest]) =>
                  runCompress(pending.paths, dest, values.format, values.level, values.password),
                )
                .catch((e: unknown) => showToast(t('op.failed', { code: errorCode(e) })))
            }}
          />
        )}
      </Suspense>

      <Suspense fallback={null}>
        {batchRename && (
          <BatchRenameDialog
            dir={batchRename.dir}
            names={batchRename.names}
            lang={lang}
            onClose={() => {
              const pending = batchRename
              setBatchRename(null)
              focusList(pending.paneId)
            }}
            onConfirm={(pairs) => {
              const dir = batchRename.dir
              const paneId = batchRename.paneId
              runOp(
                'rename',
                pairs.map((pair) => joinPath(dir, pair.from)),
                { newNames: pairs.map((pair) => pair.to) },
              )
              setBatchRename(null)
              focusList(paneId)
            }}
          />
        )}
      </Suspense>

      <Suspense fallback={null}>
        {fileTools && (
          <FileToolsPanel
            lang={lang}
            mode={fileTools.mode}
            paths={fileTools.mode === 'hash' ? fileTools.paths : []}
            root={fileTools.mode === 'dup' ? fileTools.root : ''}
            includeHidden={activePane?.hidden ?? settings.showHidden}
            dupConcurrency={settings.dupConcurrency}
            dupMaxMbPerSec={settings.dupMaxMbPerSec}
            dupExclude={settings.dupExclude}
            onDelete={(paths) => runOp('delete', paths)}
            onToast={showToast}
            onClose={() => setFileTools(null)}
          />
        )}
      </Suspense>

      <Suspense fallback={null}>
        {compareOpen && (
          <ComparePanel
            lang={lang}
            initialLeft={comparePaths[0] ?? ''}
            initialRight={comparePaths[1] ?? ''}
            openFolders={openFolders}
            includeHidden={activePane?.hidden ?? settings.showHidden}
            fontSize={settings.fontSize}
            iconSize={settings.iconSize}
            hashRateBytes={settings.dupMaxMbPerSec * 1024 * 1024}
            onClose={() => setCompareOpen(false)}
            onToast={showToast}
            onSync={runSyncActions}
          />
        )}
      </Suspense>

      {deleteConfirm && (
        <ConfirmDialog
          lang={lang}
          title={t(deleteConfirm.permanent ? 'confirm.deleteTitle' : 'confirm.recycleTitle')}
          message={t(
            deleteConfirm.permanent ? 'confirm.deletePermanent' : 'confirm.recycle',
            { count: deleteConfirm.names.length },
          )}
          detail={confirmDetail}
          confirmLabel={t(
            deleteConfirm.permanent ? 'confirm.deletePermanentAction' : 'confirm.recycleAction',
          )}
          danger
          onConfirm={() => {
            const pending = deleteConfirm
            setDeleteConfirm(null)
            focusList(pending.paneId)
            runOp('delete', pathsFor(pending.paneId, pending.names), { permanent: pending.permanent })
          }}
          onClose={() => {
            const pending = deleteConfirm
            setDeleteConfirm(null)
            if (pending) focusList(pending.paneId)
          }}
        />
      )}

      {quitBusy !== null && (
        <ConfirmDialog
          lang={lang}
          title={t('confirm.quitTitle')}
          message={t('confirm.quitBusy', { count: quitBusy })}
          confirmLabel={t('confirm.quitAnyway')}
          danger
          onConfirm={() => {
            setQuitBusy(null)
            void quitApp()
          }}
          onClose={() => setQuitBusy(null)}
        />
      )}

      <Suspense fallback={null}>
        {settingsOpen && (
          <SettingsModal
            settings={settings}
            lang={lang}
            onChange={updateSettings}
            onOpenLocation={openInApp}
            onClose={() => {
              setSettingsOpen(false)
              if (activePane) focusList(activePane.id)
            }}
          />
        )}
      </Suspense>
    </>
  )
}
