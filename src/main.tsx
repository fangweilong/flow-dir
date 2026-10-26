import { createRoot } from 'react-dom/client'
import App from './App'
import { ErrorBoundary, installErrorReporters } from './ErrorBoundary'
import './styles.css'

installErrorReporters()

createRoot(document.getElementById('root')!).render(
  <ErrorBoundary>
    <App />
  </ErrorBoundary>,
)
