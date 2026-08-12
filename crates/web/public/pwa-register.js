/**
 * Register service worker, surface updates, and expose online/offline helpers.
 * Loaded as a classic external script (CSP script-src 'self').
 */
(function () {
  'use strict';

  function setOnlineFlag(online) {
    try {
      document.documentElement.dataset.online = online ? 'true' : 'false';
      window.dispatchEvent(
        new CustomEvent('ctp-connectivity', { detail: { online: !!online } })
      );
    } catch (_) { /* ignore */ }
  }

  setOnlineFlag(typeof navigator !== 'undefined' ? navigator.onLine : true);
  window.addEventListener('online', function () { setOnlineFlag(true); });
  window.addEventListener('offline', function () { setOnlineFlag(false); });

  window.__ctpPwa = {
    applyUpdate: function () {
      var reg = window.__ctpSwRegistration;
      if (reg && reg.waiting) {
        reg.waiting.postMessage({ type: 'SKIP_WAITING' });
      }
    },
    isOnline: function () {
      return typeof navigator === 'undefined' ? true : navigator.onLine;
    },
  };

  if (!('serviceWorker' in navigator)) return;

  var isLocal =
    location.hostname === 'localhost' ||
    location.hostname === '127.0.0.1' ||
    location.hostname === '[::1]';
  if (location.protocol !== 'https:' && !isLocal) return;

  window.addEventListener('load', function () {
    navigator.serviceWorker
      .register('/sw.js', { scope: '/' })
      .then(function (reg) {
        window.__ctpSwRegistration = reg;

        function announceWaiting() {
          if (reg.waiting) {
            window.dispatchEvent(new CustomEvent('ctp-sw-update'));
          }
        }

        if (reg.waiting) announceWaiting();

        reg.addEventListener('updatefound', function () {
          var installing = reg.installing;
          if (!installing) return;
          installing.addEventListener('statechange', function () {
            if (installing.state === 'installed' && navigator.serviceWorker.controller) {
              announceWaiting();
            }
          });
        });

        // Periodic update check while tab is open.
        setInterval(function () {
          reg.update().catch(function () { /* ignore */ });
        }, 60 * 60 * 1000);
      })
      .catch(function (err) {
        console.warn('SW registration failed', err);
      });

    var refreshing = false;
    navigator.serviceWorker.addEventListener('controllerchange', function () {
      if (refreshing) return;
      refreshing = true;
      window.location.reload();
    });
  });
})();
