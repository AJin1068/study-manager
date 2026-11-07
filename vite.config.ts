import { configDefaults, defineConfig } from 'vitest/config';
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
  // Gradle copies fixtures (and their *.test.ts) under build/; generated Android projects live in gen/.
  test: {
    exclude: [...configDefaults.exclude, '**/build/**', '**/gen/**'],
  },
});
