// The remote page's service worker exists so the page can be installed as an
// app. It caches nothing: pages and assets go to the network, and the API
// (long chat turns, uploads, the event socket) never passes through it.
self.addEventListener("install", () => {
  self.skipWaiting();
});

self.addEventListener("activate", (event) => {
  event.waitUntil(self.clients.claim());
});

self.addEventListener("fetch", (event) => {
  const request = event.request;
  if (request.method !== "GET") return;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin || url.pathname.startsWith("/remote/api/")) return;
  event.respondWith(fetch(request));
});
