import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// Tauri は固定ポートを要求する (採番されると dev URL が解決できない)
const HOST = process.env.TAURI_DEV_HOST

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: HOST || false,
    hmr: HOST ? { protocol: 'ws', host: HOST, port: 5174 } : undefined,
    watch: { ignored: ['**/src-tauri/**', '**/docs/**'] },
  },
  envPrefix: ['VITE_', 'TAURI_'],
  build: {
    // WebView2 (Chromium 系) のみを対象にするため、広い互換ターゲットは不要
    target: 'chrome110',
    // Vite 8 は esbuild ではなく Oxc を使う ('esbuild' を指定すると別途 esbuild が要る)
    minify: process.env.TAURI_ENV_DEBUG ? false : 'oxc',
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
  test: {
    globals: true,
    environment: 'node',
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
  },
})
