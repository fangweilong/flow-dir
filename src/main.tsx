import { createRoot } from 'react-dom/client'
import { initPreviewToken } from './api'
import App from './App'
import { ErrorBoundary, installErrorReporters } from './ErrorBoundary'
import './styles.css'

installErrorReporters()

// The window stays hidden until the frontend reports ready, so fetching the icon/preview
// token first costs no visible time; rendering without it would leave every icon 403.
initPreviewToken()
  .catch(() => {
    // A failed IPC still renders the app; icons fall back to the theme/extension shapes.
  })
  .finally(() => {
    createRoot(document.getElementById('root')!).render(
      <ErrorBoundary>
        <App />
      </ErrorBoundary>,
    )
  })
