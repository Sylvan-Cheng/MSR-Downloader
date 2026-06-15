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
    proxy: {
      '/msr-api': {
        target: 'https://monster-siren.hypergryph.com/api',
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/msr-api/, ''),
      },
      '/msr-img': {
        target: 'https://web.hycdn.cn',
        changeOrigin: true,
        headers: {
          Referer: 'https://monster-siren.hypergryph.com/',
          'User-Agent': 'Mozilla/5.0 MSR-Downloader-GUI',
        },
        rewrite: (path) => path.replace(/^\/msr-img/, ''),
      },
    },
    watch: {
      ignored: ['**/src-tauri/**'],
    },
  },
});
