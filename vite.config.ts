import { defineConfig } from 'vite'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [tailwindcss()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { target: 'chrome126', sourcemap: false },
})
