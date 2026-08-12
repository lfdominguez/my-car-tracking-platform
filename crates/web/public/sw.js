/* Car Tracking — light shell service worker.
 * Caches static assets on successful fetch; never treats /api as offline truth.
 * Bump CACHE_VERSION when changing SW logic so clients pick up a new worker.
 */
const CACHE_VERSION = 'ctp-shell-v1';
const SHELL_CACHE = CACHE_VERSION;

const PRECACHE_URLS = [
  '/',
  '/manifest.webmanifest',
  '/icons/favicon.ico',
  '/icons/favicon-32.png',
  '/icons/apple-touch-icon.png',
  '/icons/icon-192.png',
  '/icons/icon-512.png',
  '/icons/icon-192-maskable.png',
  '/icons/icon-512-maskable.png',
  '/vendor/maplibre-gl.css',
  '/vendor/maplibre-gl.js',
  '/vendor/echarts.min.js',
  '/vendor/phosphor-duotone.css',
  '/vendor/phosphor-regular.css',
  '/qrcode.min.js',
];

self.addEventListener('install', (event) => {
  // Precache shell assets. First install activates immediately; later updates
  // stay waiting so the page can show “Update available” (SKIP_WAITING).
  event.waitUntil(
    caches
      .open(SHELL_CACHE)
      .then((cache) =>
        cache.addAll(PRECACHE_URLS.map((u) => new Request(u, { cache: 'reload' }))).catch(() => {})
      )
      .then(() => {
        if (!self.registration.active) {
          return self.skipWaiting();
        }
      })
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(keys.filter((k) => k !== SHELL_CACHE).map((k) => caches.delete(k)))
      )
      .then(() => self.clients.claim())
  );
});

self.addEventListener('message', (event) => {
  if (event.data && event.data.type === 'SKIP_WAITING') {
    self.skipWaiting();
  }
});

function isApiRequest(url) {
  return url.pathname.startsWith('/api/');
}

function isNavigationRequest(request) {
  return request.mode === 'navigate' ||
    (request.method === 'GET' && request.headers.get('accept') &&
      request.headers.get('accept').includes('text/html'));
}

function isStaticAsset(url) {
  if (url.origin !== self.location.origin) return false;
  if (isApiRequest(url)) return false;
  const p = url.pathname;
  return (
    p.startsWith('/icons/') ||
    p.startsWith('/vendor/') ||
    p.startsWith('/snippets/') ||
    p.endsWith('.js') ||
    p.endsWith('.css') ||
    p.endsWith('.wasm') ||
    p.endsWith('.png') ||
    p.endsWith('.ico') ||
    p.endsWith('.woff') ||
    p.endsWith('.woff2') ||
    p.endsWith('.webmanifest') ||
    p === '/' ||
    p.endsWith('.html')
  );
}

self.addEventListener('fetch', (event) => {
  const request = event.request;
  if (request.method !== 'GET') return;

  let url;
  try {
    url = new URL(request.url);
  } catch {
    return;
  }

  // API: network only (no offline cache of trip data).
  if (url.origin === self.location.origin && isApiRequest(url)) {
    return;
  }

  // Cross-origin (map tiles, etc.): pass through.
  if (url.origin !== self.location.origin) {
    return;
  }

  // Navigations: network first, fall back to cached shell.
  if (isNavigationRequest(request)) {
    event.respondWith(
      fetch(request)
        .then((response) => {
          const copy = response.clone();
          if (response.ok) {
            caches.open(SHELL_CACHE).then((cache) => {
              // Cache the document as navigation fallback key "/"
              cache.put('/', copy).catch(() => {});
            });
          }
          return response;
        })
        .catch(async () => {
          const cache = await caches.open(SHELL_CACHE);
          const cached =
            (await cache.match('/')) ||
            (await cache.match('/index.html')) ||
            (await cache.match(request));
          if (cached) return cached;
          return new Response('Offline', {
            status: 503,
            statusText: 'Offline',
            headers: { 'Content-Type': 'text/plain; charset=utf-8' },
          });
        })
    );
    return;
  }

  // Same-origin static: stale-while-revalidate style (cache then network update).
  if (isStaticAsset(url)) {
    event.respondWith(
      caches.open(SHELL_CACHE).then(async (cache) => {
        const cached = await cache.match(request);
        const networkPromise = fetch(request)
          .then((response) => {
            if (response && response.ok) {
              cache.put(request, response.clone()).catch(() => {});
            }
            return response;
          })
          .catch(() => null);
        if (cached) {
          networkPromise.catch(() => {});
          return cached;
        }
        const net = await networkPromise;
        if (net) return net;
        return new Response('Offline', { status: 503, statusText: 'Offline' });
      })
    );
  }
});
