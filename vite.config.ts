import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Fixed port: tauri.conf.json devUrl points here.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: 'es2021',
  },
});
