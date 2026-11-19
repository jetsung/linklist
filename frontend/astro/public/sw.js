// linklist PWA Service Worker
// 策略：仅做基本离线兜底（导航请求失败时回退缓存的起始页），
// 业务数据一律直连网络，避免书签数据出现旧缓存。对齐上游 PWA 的"可安装"目标。
const CACHE_NAME = 'linklist-shell-v1';
const OFFLINE_URLS = ['/bookmarks', '/logo-512.png', '/icons/icon-192.png', '/icons/icon-512.png'];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches.open(CACHE_NAME).then((cache) => cache.addAll(OFFLINE_URLS)).then(() => self.skipWaiting())
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE_NAME).map((k) => caches.delete(k))))
      .then(() => self.clients.claim())
  );
});

self.addEventListener('fetch', (event) => {
  const req = event.request;
  if (req.method !== 'GET') return;
  const url = new URL(req.url);
  if (url.origin !== self.location.origin) return;
  // API 请求不缓存（书签数据必须新鲜）。
  if (url.pathname.startsWith('/api/')) return;

  // 页面导航：网络优先，失败回退缓存页（离线兜底）。
  if (req.mode === 'navigate') {
    event.respondWith(
      fetch(req)
        .then((res) => {
          const copy = res.clone();
          caches.open(CACHE_NAME).then((cache) => cache.put(req, copy)).catch(() => {});
          return res;
        })
        .catch(() => caches.match(req).then((hit) => hit || caches.match('/bookmarks')))
    );
    return;
  }

  // 静态资源：缓存优先（构建产物带指纹，安全）。
  event.respondWith(
    caches.match(req).then(
      (hit) =>
        hit ||
        fetch(req).then((res) => {
          if (res.ok && (url.pathname.startsWith('/assets/') || url.pathname.startsWith('/icons/'))) {
            const copy = res.clone();
            caches.open(CACHE_NAME).then((cache) => cache.put(req, copy)).catch(() => {});
          }
          return res;
        })
    )
  );
});
