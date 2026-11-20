import { defineConfig } from 'vite';

// The browser only ever talks to the BFF. In development Vite serves the page and its modules
// and passes everything else through to the BFF; it never reaches the backend on 4101.
const bff = process.env.STRATA_BFF_URL ?? 'http://127.0.0.1:4100';

export default defineConfig({
  root: import.meta.dirname,
  server: {
    host: '127.0.0.1',
    port: Number(process.env.STRATA_VITE_PORT ?? 5173),
    strictPort: true,
    // The BFF picks the tenant from the Host header, so the original host is passed on.
    proxy: {
      '/api': { target: bff, changeOrigin: false },
      '/federation': { target: bff, changeOrigin: false },
      '/ws': { target: bff, changeOrigin: false, ws: true },
    },
    allowedHosts: ['localhost', 'tidewater.localhost'],
  },
  worker: { format: 'es' },
  // The module-preload polyfill is left out: es2022 browsers preload natively, and the page's
  // Trusted Types policy leaves no room for a helper script the build injects.
  build: { outDir: 'dist', emptyOutDir: true, target: 'es2022', sourcemap: false, modulePreload: { polyfill: false } },
});
