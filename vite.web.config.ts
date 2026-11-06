import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Static, host-agnostic web bundle (profile link and dashboard view). Run from the repository
// root: `pnpm build:web`. Output: web/dist, served from any path (relative base, hash routes).
// Runtime settings come from config.json next to index.html (public URL and anon key only).
export default defineConfig({
  root: 'web',
  base: './',
  plugins: [react()],
  clearScreen: false,
  build: {
    target: 'es2022',
    outDir: 'dist',
    emptyOutDir: true,
  },
});
