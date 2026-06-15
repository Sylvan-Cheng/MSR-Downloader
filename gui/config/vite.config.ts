import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';
import { resolve } from 'node:path';

const guiRoot = resolve(__dirname, '..');

export default defineConfig({
  root: guiRoot,
  plugins: [svelte({ configFile: resolve(__dirname, 'svelte.config.js') })],
  clearScreen: false,
  css: {
    postcss: resolve(__dirname, 'postcss.config.cjs'),
  },
  build: {
    outDir: resolve(guiRoot, '..', 'dist'),
    emptyOutDir: true,
  },
  server: {
    host: '127.0.0.1',
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ['**/src-tauri/**'],
    },
  },
});
