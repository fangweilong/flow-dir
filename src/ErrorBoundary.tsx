import { Component } from 'react'
import type { ErrorInfo, ReactNode } from 'react'
import { logDirPath, logFrontend, openPath } from './api'
import { makeT } from './i18n'

type Props = { children: ReactNode }
type State = { error: Error | null }

/**
 * Last line of defence for render errors: without it a thrown component leaves a white
 * window and the user has nothing to report. Recovery is explicit (reload) and the log
 * folder is one click away because the report needs the file.
 */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null }

  static getDerivedStateFromError(error: Error): State {
    return { error }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    logFrontend(
      'error',
      `render: ${error.message}`,
      `${error.stack ?? ''}\n${info.componentStack ?? ''}`,
    )
  }

  render() {
    if (!this.state.error) return this.props.children
    const t = makeT('zh-CN')
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 bg-surface-2 p-6 text-fg">
        <div className="max-w-[32rem] text-center">
          <h1 className="text-[0.9375rem] font-semibold">{t('crash.title')}</h1>
          <p className="mt-1 text-[0.75rem] text-muted">{t('crash.hint')}</p>
          <pre className="mt-2 max-h-40 overflow-auto rounded border border-line bg-surface p-2 text-left font-mono text-[0.6875rem] text-muted">
            {this.state.error.message}
          </pre>
        </div>
        <div className="flex gap-2">
          <button
            type="button"
            onClick={() => window.location.reload()}
            className="rounded-md border border-line px-3 py-1 text-[0.75rem] text-fg hover:bg-surface-3"
          >
            {t('crash.reload')}
          </button>
          <button
            type="button"
            onClick={() =>
              void logDirPath()
                .then((dir) => (dir ? openPath(dir) : undefined))
                .catch(() => undefined)
            }
            className="rounded-md border border-line px-3 py-1 text-[0.75rem] text-fg hover:bg-surface-3"
          >
            {t('crash.openLogs')}
          </button>
        </div>
      </div>
    )
  }
}

/** Uncaught errors and rejected promises go to the backend log, deduped there by caller. */
export function installErrorReporters() {
  window.addEventListener('error', (event) => {
    logFrontend(
      'error',
      event.message || 'window error',
      `${event.filename}:${event.lineno}:${event.colno}\n${event.error?.stack ?? ''}`,
    )
  })
  window.addEventListener('unhandledrejection', (event) => {
    const reason = event.reason
    logFrontend(
      'error',
      'unhandled rejection',
      reason instanceof Error ? (reason.stack ?? reason.message) : String(reason),
    )
  })
}
