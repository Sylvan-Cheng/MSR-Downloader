import type { Config } from 'tailwindcss';

export default {
  content: ['./gui/index.html', './gui/src/**/*.{svelte,ts}'],
  theme: {
    extend: {
      colors: {
        graphite: '#09090b',
        panel: '#111317',
        amberline: '#d4d8dd',
      },
      fontFamily: {
        sans: ['Microsoft YaHei', 'Segoe UI', 'Helvetica Neue', 'Arial', 'ui-sans-serif', 'system-ui', 'sans-serif'],
      },
    },
  },
  plugins: [],
} satisfies Config;
