import { defineConfig } from 'vite';

// Run from the repository root (`pnpm build:extension`). Unpacked load target: extension/dist
// (manifest.json is copied from extension/public).
export default defineConfig({
  root: 'extension',
  base: './',
  clearScreen: false,
  build: {
    target: 'es2022',
    outDir: 'dist',
    emptyOutDir: true,
    rollupOptions: {
      input: {
        background: 'src/background.ts',
        blocked: 'blocked.html',
      },
      output: {
        entryFileNames: (chunk) => (chunk.name === 'background' ? 'background.js' : 'assets/[name]-[hash].js'),
      },
    },
  },
});
