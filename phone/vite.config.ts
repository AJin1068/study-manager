import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// The phone UI is its own Vite root (phone/), run from the repository root with
// `vite --config phone/vite.config.ts`. Fixed port: phone/src-tauri/tauri.conf.json devUrl.
export default defineConfig({
  root: fileURLToPath(new URL('.', import.meta.url)),
  plugins: [react()],
  clearScreen: false,
  server: { port: 1421, strictPort: true, host: true },
  build: { target: 'es2021', outDir: 'dist', emptyOutDir: true },
});
