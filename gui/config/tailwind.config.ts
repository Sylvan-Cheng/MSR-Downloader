import type { Config } from 'tailwindcss';

export default {
  content: ['./gui/index.html', './gui/src/**/*.{svelte,ts}'],
  theme: {
    extend: {
      colors: {
        graphite: '#101416',
        panel: '#161b1d',
        amberline: '#d6a85e',
      },
      fontFamily: {
        sans: ['Inter', 'ui-sans-serif', 'system-ui', 'sans-serif'],
      },
    },
  },
  plugins: [],
} satisfies Config;
