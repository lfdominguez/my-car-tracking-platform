/* Car Tracking — light shell service worker.
 * Caches static assets on successful fetch; never treats /api as offline truth.
 *
 * BUILD_ID is replaced per build by the post_build hook in crates/web/Trunk.toml,
 * so each deploy ships a new worker whose activate step drops the previous cache.
 * Without the hook (e.g. a hand-copied dist) the placeholder stays and the
 * activate-time prune below still evicts hashed assets the live index no longer
 * references. Bump SW_LOGIC_VERSION when changing this file's logic.
 */
const SW_LOGIC_VERSION = 'v2';
const BUILD_ID = '__CTP_BUILD_ID__';
const CACHE_VERSION = `ctp-shell-${SW_LOGIC_VERSION}-${BUILD_ID}`;
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

/**
 * Trunk output names carry a content hash (`web-1a2b3c4d5e6f7a8b.js`, `..._bg.wasm`,
 * `style-<hash>.css`), and wasm-bindgen snippets live under a hashed directory
 * (`/snippets/web-<hash>/inline0.js`).
 */
function isHashedAsset(pathname) {
  return pathname.startsWith('/snippets/') || /-[0-9a-f]{12,}(_bg)?\.(js|wasm|css)$/.test(pathname);
}

/**
 * Evict hashed assets the live index.html no longer references. Each deploy adds a
 * new web-<hash>.js / _bg.wasm pair; under an unchanged cache name they piled up
 * forever. Best effort: offline or a failed fetch leaves the cache as it is.
 */
async function pruneStaleHashedAssets() {
  let html;
  try {
    const res = await fetch('/', { cache: 'no-store' });
    if (!res.ok) return;
    html = await res.text();
  } catch (_) {
    return;
  }
  const cache = await caches.open(SHELL_CACHE);
  const requests = await cache.keys();
  await Promise.all(
    requests.map((req) => {
      const path = new URL(req.url).pathname;
      if (isHashedAsset(path) && !html.includes(path)) {
        return cache.delete(req);
      }
      return null;
    })
  );
}

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(keys.filter((k) => k !== SHELL_CACHE).map((k) => caches.delete(k)))
      )
      .then(() => pruneStaleHashedAssets())
      .catch(() => {})
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

  // Hashed build output never changes under its name: cache first, no revalidation.
  if (isHashedAsset(url.pathname)) {
    event.respondWith(
      caches.open(SHELL_CACHE).then(async (cache) => {
        const cached = await cache.match(request);
        if (cached) return cached;
        try {
          const response = await fetch(request);
          if (response && response.ok) {
            cache.put(request, response.clone()).catch(() => {});
          }
          return response;
        } catch (_) {
          return new Response('Offline', { status: 503, statusText: 'Offline' });
        }
      })
    );
    return;
  }

  // Unhashed same-origin static (/vendor, icons, fonts, sw-adjacent files):
  // stale-while-revalidate — serve the cached copy, refresh it in the background.
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
