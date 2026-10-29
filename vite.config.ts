import { defineConfig } from 'vite'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [tailwindcss()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: {
    target: 'chrome126',
    sourcemap: false,
    chunkSizeWarningLimit: 500,
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (id.includes('node_modules')) {
            if (id.includes('react') || id.includes('react-dom')) {
              return 'vendor-react'
            }
            if (id.includes('@tanstack/react-virtual')) {
              return 'vendor-virtual'
            }
            if (id.includes('@tauri-apps')) {
              return 'vendor-tauri'
            }
            return 'vendor-misc'
          }
        },
      },
    },
  },
})
