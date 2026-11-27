// The family's service worker (Kit.offline() registers it from /sw.js, scope /):
//
// - The offline snapshot. Every same-origin GET goes to the network first; a good answer is
//   kept (with when, in X-Kit-Saved). When the network fails or takes over 5 s (the tailnet
//   is off, this machine sleeps), the kept answer comes back instead, and the page is told
//   ({kit: "offline", saved}) so Kit.offline() shows "Offline · as of 08:12" until an
//   answer comes from the network again ({kit: "online"}). Never kept: event streams, ranges
//   (audio), /api/push* (under a mount too: /lumon/api/push), anything not 200, and an
//   answer marked Cache-Control: no-store. The URL is the key, query and all: a cache-buster
//   (?fresh=1) gets a snapshot of its own, so send it as a header instead. A page opened
//   offline that was never kept (the home-screen icon's /, which redirects) gets the last
//   page that was.
// - Push where the message isn't shown by the phone itself. An iPhone (18.4+) shows the
//   kit's declarative messages without waking this worker; anywhere else the push event
//   gets the same JSON ({web_push, notification: {title, body, navigate, tag}, app_badge})
//   and shows it here, and a tap opens notification.navigate.
//
// The kit's (~/Work/rb/ghostwheel/kit/web/sw.js), copied into each app by kit/sync.sh:
// change it there, not in a copy.
"use strict"

var CACHE = "kit-snapshot-v1", WAIT_MS = 5000, LAST_PAGE = "/__kit/last-page"

self.addEventListener("install", function() { self.skipWaiting() })
self.addEventListener("activate", function(e) { e.waitUntil(self.clients.claim()) })

function keepable(req, url) {
  if (req.method !== "GET" || url.origin !== self.location.origin) return false
  if (req.headers.has("range") || /text\/event-stream/.test(req.headers.get("accept") || "")) return false
  if (req.destination === "audio" || req.destination === "video") return false
  return !/(^|\/)api\/push(\/|$)/.test(url.pathname)
}

function tell(e, msg) {
  var id = e.clientId || e.resultingClientId
  if (!id) return
  // a navigation's page isn't there yet: tell it once it is
  var tries = 0
  ;(function again() {
    self.clients.get(id).then(function(c) {
      if (c) c.postMessage(msg)
      else if (++tries < 20) setTimeout(again, 250)
    })
  })()
}

function keep(req, res) {
  return res.clone().blob().then(function(body) {
    var h = new Headers(res.headers)
    h.set("X-Kit-Saved", String(Date.now()))
    var init = { status: res.status, statusText: res.statusText, headers: h }
    return caches.open(CACHE).then(function(c) {
      var jobs = [c.put(req, new Response(body, init))]
      if (req.mode === "navigate") jobs.push(c.put(new Request(self.location.origin + LAST_PAGE), new Response(body, init)))
      return Promise.all(jobs)
    })
  }).catch(function() {})
}

self.addEventListener("fetch", function(e) {
  var req = e.request, url = new URL(req.url)
  if (!keepable(req, url)) return
  var net = fetch(req).then(function(res) {
    if (res.ok && res.type === "basic") {
      // no-store means kept nowhere: a health record (Здраве) is never in the snapshot
      if (!/no-store/i.test(res.headers.get("Cache-Control") || "")) e.waitUntil(keep(req, res))
      if (/\/api\//.test(url.pathname) || req.mode === "navigate") tell(e, { kit: "online" })
    }
    return res
  })
  net.catch(function() {}) // answered from the snapshot: the network's failure is no news
  var timer
  var late = new Promise(function(_, fail) { timer = setTimeout(function() { fail(new Error("slow")) }, WAIT_MS) })
  e.respondWith(Promise.race([net, late]).then(function(res) { clearTimeout(timer); return res }, function(err) {
    clearTimeout(timer)
    return caches.match(req, { ignoreVary: true }).then(function(kept) {
      return kept || (req.mode === "navigate" ? caches.match(self.location.origin + LAST_PAGE) : null)
    }).then(function(kept) {
      if (!kept) return net // nothing kept: the network's answer or its failure, whenever it comes
      tell(e, { kit: "offline", saved: Number(kept.headers.get("X-Kit-Saved")) || null })
      return kept
    })
  }))
})

self.addEventListener("push", function(e) {
  var m = {}
  try { m = e.data ? e.data.json() : {} } catch (err) { m = { notification: { title: e.data ? e.data.text() : "" } } }
  var n = m.notification || {}, badge = m.app_badge, nav = self.navigator
  var jobs = [self.registration.showNotification(n.title || "", { body: n.body || "", tag: n.tag, data: { navigate: n.navigate || "/" } })]
  if (badge != null && nav && nav.setAppBadge) jobs.push(Number(badge) > 0 ? nav.setAppBadge(Number(badge)) : nav.clearAppBadge())
  e.waitUntil(Promise.all(jobs).catch(function() {}))
})

self.addEventListener("notificationclick", function(e) {
  e.notification.close()
  var to = new URL((e.notification.data && e.notification.data.navigate) || "/", self.location.origin).href
  e.waitUntil(self.clients.matchAll({ type: "window", includeUncontrolled: true }).then(function(cs) {
    for (var i = 0; i < cs.length; i++) {
      if (new URL(cs[i].url).origin === self.location.origin && cs[i].navigate) return cs[i].navigate(to).then(function(c) { return c && c.focus() })
    }
    return self.clients.openWindow(to)
  }))
})
