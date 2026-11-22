// The family's page helpers (Ghostwheel's, which Lumon, Spanreed, Macrodata and
// Ironforge share), as window.Kit:
//   Kit.loadTheme()   GET /api/theme (the Omarchy theme's colors.toml) and apply it:
//                     the tokens kit.css defines, the color scheme (and <html
//                     data-scheme="dark|light"), the theme-color;
//                     resolves to {theme, dark}, or null when there's none
//   Kit.applyTheme(t), Kit.themeVars(t)   the same for a theme in hand (themeVars is pure)
//   Kit.goodColor / warnColor / urgentColor   the theme's green / yellow / red when it really is one and
//                     reads on the background, else one for a light or dark page
//   Kit.onAccent(hex)   the text colour on the accent (--on-accent: white or near-black)
//   Kit.contrastOf(a, b)   WCAG's contrast ratio of two hex colours
//   Kit.readable(c, bg, fg, min)   c as it is when it has min contrast on bg, else moved toward fg
//                     until it has (themeVars: --dim 4.5:1, --muted 3:1)
//   Kit.h(tag, attrs, kids)   an element: strings and numbers become text nodes, never
//                     HTML; an attribute that's a function is a listener (onclick)
//   Kit.pullToRefresh(onRefresh)   pull down from the top to call onRefresh (touch only:
//                     a no-op on desktop); a ring under the safe area shows the pull and
//                     turns until onRefresh's promise settles. With autoUpdate on it first asks for a
//                     newer page (updateNow) and reloads instead when there is one. Starts only when the page
//                     and every scroller under the finger are at their top, and never in
//                     [data-no-pull]; kit.css's .kit-ptr draws it
//   Kit.pullTravel(dy)   how far the ring follows a finger dy px down (pure)
// The phone (the home-screen app; each a no-op where the browser hasn't got it):
//   Kit.offline(onChange)   register /sw.js (kit/web/sw.js: the offline snapshot, push)
//                     and show "Offline · as of 08:12" (.kit-offline) while answers come
//                     from the snapshot; Kit.fmtSaved(ms, now) words it (pure)
//   Kit.autoUpdate({every})   reload when the page changed on the server: a HEAD of it every
//                     minute and when the app comes back to the front, its X-Kit-Page against
//                     the page's <meta name="kit-page"> (the crate's server::file puts both
//                     on); never while a dialog is open or a field has focus, and once per
//                     version. Kit.pageChanged(have, now, ok, reloadedFor) decides (pure).
//                     Kit.updateNow() asks at once (a pull to refresh does, first) and
//                     resolves true when it's reloading
//   Kit.push.state() / enable() / disable()   push to this phone through /api/push* (the
//                     crate's push::Push::route); enable() from a tap. Kit.pushButton() is
//                     a header .btn.icon (a bell) that shows and flips it
//   Kit.badge(n)      the number on the app's icon (0 clears it)
//   Kit.tick()        a haptic tick (iOS: a switch toggled; elsewhere vibrate), from a tap
//   Kit.share(data)   the share sheet ({title, text, url, files}), else the clipboard
//   Kit.transition(update)   update the page through a View Transition where there is one
//   Kit.keyboard()    the on-screen keyboard's height in --kb, html.kit-kb; .kit-dock rides it
//   Kit.swipe(row, {left, right})   swipe a row for an action ({label, color, run(row)}):
//                     ticks at Kit.SWIPE_AT px, runs when let go; Kit.swipeTravel (pure)
//   Kit.choice({options, value, onChange})   a row of chips, one picked ({el, set, value})
//   Kit.autogrow(textarea, max?)   a textarea that grows with its text, up to max px
//   Kit.fling(card, {left, right})   a card that follows the finger (or the mouse) and
//                     tilts, with a stamp for each side ({label, color, run(card)}); let go
//                     past Kit.FLING_AT px (or flicked) it flies off that way and runs.
//                     Returns {go(side)}: the same fling from a key or a button. Kit.flingTilt (pure)
//   Kit.sheet({title, text, actions: [{label, danger, run}]})   a bottom sheet of
//                     actions; resolves to the label chosen, or null; dragged down it
//                     closes. A page's own sheet gets the same from Kit.sheetDrag(box, close),
//                     and Kit.sheetOpen(dlg) shows it rising from the bottom
//   Kit.privacy()     blur the page while the app is in the background (the app switcher)
//   Kit.lock({app, after})   Face ID before the page shows, and after `after` min away
//   Kit.standalone()  whether it runs as the home-screen app
// Plain script, no build: the page loads /theme.js before its own script (or has it inline:
// so no closing script tag in here).
//
// The kit's (~/Work/rb/ghostwheel/kit/web/theme.js), copied into each app by
// kit/sync.sh: change it there, not in a copy.
;(function(root) {
"use strict"

var GOOD_LIGHT = "#2f9e44", GOOD_DARK = "#69db7c", WARN_LIGHT = "#c27a12", WARN_DARK = "#e0a84a"
var URGENT_LIGHT = "#c0392b", URGENT_DARK = "#ff6b6b"
var BLACK = { r: 0, g: 0, b: 0 }, WHITE = { r: 1, g: 1, b: 1 }

function rgbOf(x) {
  var m = /^#?([0-9a-fA-F]{6})$/.exec(String(x || "").trim())
  if (!m) return null
  var n = parseInt(m[1], 16)
  return { r: (n >> 16 & 255) / 255, g: (n >> 8 & 255) / 255, b: (n & 255) / 255 }
}
function hex(c) {
  function two(v) { var s = Math.round(v * 255).toString(16); return s.length < 2 ? "0" + s : s }
  return "#" + two(c.r) + two(c.g) + two(c.b)
}
// a, p of the way to b
function mix(a, b, p) { return { r: a.r + (b.r - a.r) * p, g: a.g + (b.g - a.g) * p, b: a.b + (b.b - a.b) * p } }
function luminance(c) {
  function lin(v) { return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4) }
  return 0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}
function contrast(a, b) { var x = luminance(a), y = luminance(b); return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05) }
function hue(c) {
  var max = Math.max(c.r, c.g, c.b), min = Math.min(c.r, c.g, c.b), d = max - min
  if (max === 0 || d === 0) return { h: 0, s: 0 }
  var h = max === c.r ? 60 * (((c.g - c.b) / d) % 6) : max === c.g ? 60 * ((c.b - c.r) / d + 2) : 60 * ((c.r - c.g) / d + 4)
  return { h: h < 0 ? h + 360 : h, s: d / max }
}
// Hue 70°-165° with an HSV saturation of 0.2 or more
function isGreen(c) { var x = hue(c); return x.s >= 0.2 && x.h >= 70 && x.h <= 165 }
// Hue 25°-65° with an HSV saturation of 0.35 or more
function isAmber(c) { var x = hue(c); return x.s >= 0.35 && x.h >= 25 && x.h <= 65 }
// Hue 290°-30° (wrapping through 0°: magenta reds count, as lupine's) with an HSV saturation of 0.35 or more
function isRed(c) { var x = hue(c); return x.s >= 0.35 && (x.h >= 290 || x.h <= 30) }
function pick(themed, bgHex, isHue, light, dark) {
  var bg = rgbOf(bgHex), c = rgbOf(themed)
  if (c && isHue(c) && (!bg || contrast(c, bg) >= 2.2)) return String(themed).toLowerCase()
  return !bg || luminance(bg) < 0.4 ? dark : light
}
function goodColor(green, bgHex) { return pick(green, bgHex, isGreen, GOOD_LIGHT, GOOD_DARK) }
function warnColor(yellow, bgHex) { return pick(yellow, bgHex, isAmber, WARN_LIGHT, WARN_DARK) }
function urgentColor(red, bgHex) { return pick(red, bgHex, isRed, URGENT_LIGHT, URGENT_DARK) }
// Text on the accent (a primary button): white or near-black, whichever reads better
function onAccent(accentHex) {
  var a = rgbOf(accentHex)
  if (!a) return "#fff"
  return contrast(a, WHITE) >= contrast(a, BLACK) ? "#fff" : "#141414"
}

// The least contrast on the page's background: --dim is text you read (the meta lines, a tally),
// so WCAG AA's 4.5:1; --muted is what you only glance at (a label, a placeholder), 3:1
var DIM_MIN = 4.5, MUTED_MIN = 3
// The contrast of two hex colours (WCAG's ratio, 1 to 21); 0 when one isn't a colour
function contrastOf(aHex, bHex) { var a = rgbOf(aHex), b = rgbOf(bHex); return a && b ? contrast(a, b) : 0 }
// A theme's grey made readable: as it is when it has min contrast on bg, else moved toward the
// foreground until it has (Rosé Pine Dawn's muted #cecacd is 1.5:1 on its #faf4ed)
function readable(colorHex, bgHex, fgHex, min) {
  var c = rgbOf(colorHex), bg = rgbOf(bgHex), fg = rgbOf(fgHex)
  if (!c || !bg || !fg || contrast(c, bg) >= min) return colorHex
  for (var p = .05; p < 1; p += .05) { var m = mix(c, fg, p); if (contrast(m, bg) >= min) return hex(m) }
  return hex(fg)
}

// The tokens for /api/theme's answer (colors.toml's keys): {dark, vars}, null without a background
function themeVars(t) {
  if (!t || !t.background) return null
  var bg = rgbOf(t.background), dark = bg ? luminance(bg) < .4 : t.mode === "dark"
  var ink = dark ? "255,255,255" : "0,0,0", v = {}
  v["--bg"] = t.background
  if (t.foreground) v["--fg"] = t.foreground
  var fg = t.foreground || (dark ? "#ffffff" : "#000000")
  if (t.muted) v["--muted"] = readable(t.muted, t.background, fg, MUTED_MIN)
  var dim = t.dark_foreground || t.light_foreground || t.muted
  if (dim) v["--dim"] = readable(dim, t.background, fg, DIM_MIN)
  if (t.accent) v["--accent"] = t.accent
  v["--urgent"] = urgentColor(t.red, t.background)
  v["--good"] = goodColor(t.green, t.background)
  v["--warn"] = warnColor(t.yellow, t.background)
  v["--line"] = "rgba(" + ink + ",.1)"
  v["--chip"] = "rgba(" + ink + (dark ? ",.07)" : ",.045)")
  // a card's ground: a shade off the page, opaque (sticky cells sit on it)
  if (bg) v["--surface"] = hex(mix(bg, dark ? WHITE : BLACK, dark ? .04 : .03))
  var a = rgbOf(t.accent)
  if (a) v["--on-accent"] = onAccent(t.accent)
  if (a) v["--sel"] = "rgba(" + [a.r, a.g, a.b].map(function(x) { return Math.round(x * 255) }).join(",") + ",.13)"
  return { dark: dark, vars: v }
}

function applyTheme(t) {
  var th = themeVars(t)
  if (!th) return null
  var s = document.documentElement.style
  for (var k in th.vars) s.setProperty(k, th.vars[k])
  s.colorScheme = th.dark ? "dark" : "light"
  document.documentElement.dataset.scheme = th.dark ? "dark" : "light" // for CSS: :root[data-scheme=dark]
  var m = /** @type {HTMLMetaElement | null} */ (document.querySelector('meta[name="color-scheme"]')); if (m) m.content = th.dark ? "dark" : "light"
  var tc = /** @type {HTMLMetaElement | null} */ (document.querySelector('meta[name="theme-color"]')); if (tc) tc.content = t.background
  return th
}

function loadTheme(url) {
  return fetch(url || "/api/theme", { cache: "no-store" })
    .then(function(r) { return r.ok ? r.json() : null })
    .then(function(t) { var th = applyTheme(t); return th ? { theme: t, dark: th.dark } : null })
    .catch(function() { return null })
}

function h(tag, attrs, kids) {
  var el = document.createElement(tag)
  for (var k in attrs || {}) {
    var v = attrs[k]
    if (v == null || v === false) continue
    if (typeof v === "function") el.addEventListener(k.slice(2), v)
    else el.setAttribute(k, v === true ? "" : v)
  }
  ;(function add(c) {
    if (c == null || c === false) return
    if (Array.isArray(c)) return c.forEach(add)
    el.appendChild(typeof c === "string" || typeof c === "number" ? document.createTextNode(String(c)) : c)
  })(kids)
  return el
}

// Pull-to-refresh, Lumon's and Ghostwheel's made one. A home-screen web app has no reload
// button, so the pull is how the phone asks again.
var PULL_AT = 64, PULL_MAX = 96
function pullTravel(dy) { return dy <= 0 ? 0 : Math.min(PULL_MAX, dy * 0.5) }

function pullToRefresh(onRefresh) {
  var w = root, doc = w.document
  if (!doc || !("ontouchstart" in w) || !w.matchMedia || !w.matchMedia("(any-pointer: coarse)").matches) return null
  var ring = doc.createElement("div")
  ring.className = "kit-ptr"
  ring.setAttribute("aria-hidden", "true")
  ring.appendChild(doc.createElement("i"))
  doc.body.appendChild(ring)
  var x0 = 0, owned = false, travel = 0, busy = false
  /** @type {number | null} */
  var y0 = null

  function show(d, animate) {
    ring.classList.toggle("drag", !animate)
    ring.classList.toggle("ready", d >= PULL_AT)
    ring.style.transform = "translate(-50%," + (d - 48) + "px)"
    ring.style.opacity = d <= 0 ? "0" : String(Math.min(1, d / PULL_AT))
    var dot = /** @type {HTMLElement} */ (ring.firstChild)
    dot.style.transform = "rotate(" + Math.round(d / PULL_AT * 270) + "deg)"
  }
  // the page, and every scroller between it and the finger, at its top
  function atTop(el) {
    if ((w.scrollY || doc.documentElement.scrollTop) > 0) return false
    for (; el && el.nodeType === 1 && el !== doc.body; el = el.parentNode) {
      if (el.hasAttribute("data-no-pull")) return false
      if (el.scrollTop > 0 && el.scrollHeight > el.clientHeight) return false
    }
    return true
  }
  function reset() { y0 = null; owned = false; travel = 0 }

  doc.addEventListener("touchstart", function(e) {
    reset()
    // a sheet or dialog is up: a pull is for it (sheetDrag), not the page under it
    if (busy || e.touches.length !== 1 || doc.querySelector("dialog[open]") || !atTop(e.target)) return
    x0 = e.touches[0].clientX; y0 = e.touches[0].clientY
  }, { passive: true })
  doc.addEventListener("touchmove", function(e) {
    if (y0 == null) return
    if (e.touches.length !== 1) { reset(); show(0, true); return }
    var dx = e.touches[0].clientX - x0, dy = e.touches[0].clientY - y0
    if (!owned) {
      if (Math.abs(dx) < 6 && Math.abs(dy) < 6) return
      // up or sideways first: a scroll or a swipe, not ours
      if (dy <= 0 || Math.abs(dx) >= dy) { reset(); return }
      owned = true
    }
    // ours from here: no rubber band under the ring
    if (e.cancelable) e.preventDefault()
    travel = pullTravel(dy)
    show(travel, false)
  }, { passive: false })
  function end() {
    if (!owned) { reset(); return }
    var go = travel >= PULL_AT
    reset()
    if (!go) { show(0, true); return }
    busy = true
    ring.classList.add("busy")
    show(PULL_AT * 0.75, true)
    var t0 = Date.now(), p
    // a newer page first: then the pull reloads the app (the ring spins until it does)
    p = updateNow().then(function(reloading) { return reloading ? new Promise(function() {}) : onRefresh() })
    Promise.resolve(p).catch(function() {}).then(function() {
      setTimeout(function() { busy = false; ring.classList.remove("busy"); show(0, true) }, Math.max(0, 500 - (Date.now() - t0)))
    })
  }
  doc.addEventListener("touchend", end)
  doc.addEventListener("touchcancel", function() { reset(); show(0, true) })
  show(0, true)
  return ring
}

// ---- The phone: the home-screen app's offline snapshot, push, badge, and native feel ----

function fmtSaved(ms, now) {
  if (!ms) return "Offline"
  var d = new Date(ms), n = now ? new Date(now) : new Date()
  function two(x) { return x < 10 ? "0" + x : String(x) }
  var hm = two(d.getHours()) + ":" + two(d.getMinutes())
  if (d.toDateString() === n.toDateString()) return "Offline · as of " + hm
  return "Offline · as of " + ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][d.getDay()] + " " + d.getDate() + " " + hm
}

// Register /sw.js (the offline snapshot and push) and show "Offline · as of 08:12" while
// answers come from the snapshot. onChange(offline, saved) is told each time that turns.
// Resolves to the registration, or null where there are no service workers (plain http
// other than localhost, an old browser).
function offline(onChange) {
  var nav = root.navigator, doc = root.document
  if (!nav || !("serviceWorker" in nav)) return Promise.resolve(null)
  /** @type {HTMLDivElement | null} */
  var bar = null
  var down = false
  function set(isDown, saved) {
    if (!bar) {
      bar = doc.createElement("div")
      bar.className = "kit-offline"
      bar.setAttribute("role", "status")
      doc.body.appendChild(bar)
    }
    if (isDown) bar.textContent = fmtSaved(saved)
    doc.documentElement.classList.toggle("kit-is-offline", isDown)
    if (isDown !== down && onChange) onChange(isDown, saved || null)
    down = isDown
  }
  nav.serviceWorker.addEventListener("message", function(e) {
    var m = e.data || {}
    if (m.kit === "offline") set(true, m.saved)
    else if (m.kit === "online" && down) set(false)
  })
  root.addEventListener("online", function() { if (down) set(false) })
  return nav.serviceWorker.register("/sw.js", { scope: "/" }).catch(function() { return null })
}

// Whether a page tagged `have` should reload for the tag `now` a HEAD of it answered (`ok`):
// a different one, and not one it already reloaded for (`reloadedFor`), which a page that
// comes back the same (an offline snapshot, a slow network) would otherwise loop on
function pageChanged(have, now, ok, reloadedFor) {
  return !!(ok && have && now && now !== have && now !== reloadedFor)
}

// Reload when the page has changed on the server (the module header). False when the page
// carries no version (served by something that doesn't stamp it)
/** @type {((asked: boolean) => Promise<boolean>) | null} */
var pageCheck = null
function autoUpdate(opts) {
  var doc = root.document, loc = root.location
  var meta = doc && doc.querySelector('meta[name="kit-page"]')
  if (!meta || !root.fetch) return false
  var have = meta.getAttribute("content"), busy = false, key = "kit-page-reloaded"
  function stored() { try { return root.sessionStorage.getItem(key) } catch (e) { return null } }
  function engaged() {
    if (doc.querySelector("dialog[open]")) return true
    var a = /** @type {HTMLElement | null} */ (doc.activeElement)
    return !!a && (/^(INPUT|TEXTAREA|SELECT)$/.test(a.tagName) || a.isContentEditable)
  }
  // `asked`: from a pull or a button, so a version already reloaded for is tried again (a
  // reload that came back stale, from the offline snapshot, isn't stuck until the next one)
  function check(asked) {
    if (busy || doc.visibilityState !== "visible") return Promise.resolve(false)
    busy = true
    return root.fetch(loc.pathname + loc.search, { method: "HEAD", cache: "no-store" }).then(function(r) {
      var now = r.headers.get("X-Kit-Page")
      if (!pageChanged(have, now, r.ok, asked ? null : stored()) || (!asked && engaged())) return false
      try { root.sessionStorage.setItem(key, String(now)) } catch (e) {}
      loc.reload()
      return true
    }).catch(function() { return false }).then(function(reloading) { busy = false; return reloading })
  }
  pageCheck = check
  var auto = function() { check(false) }
  root.setInterval(auto, (opts && opts.every) || 60000)
  doc.addEventListener("visibilitychange", auto)
  // an iPhone brings a home-screen app back with pageshow (from its page cache) or focus,
  // not always a visibilitychange
  root.addEventListener("pageshow", function(e) { if (e.persisted) auto() })
  root.addEventListener("focus", auto)
  return true
}

// Ask now whether the page changed, and reload if it did: true when it's reloading. What
// pullToRefresh does before its own refresh; also for a page's refresh button
function updateNow() { return pageCheck ? pageCheck(true) : Promise.resolve(false) }

function b64urlBytes(s) {
  var b = atob(s.replace(/-/g, "+").replace(/_/g, "/") + "===".slice((s.length + 3) % 4))
  var out = new Uint8Array(b.length)
  for (var i = 0; i < b.length; i++) out[i] = b.charCodeAt(i)
  return out
}

function api(method, path, body) {
  return fetch(path, { method: method, cache: "no-store", headers: body ? { "Content-Type": "application/json" } : {}, body: body ? JSON.stringify(body) : undefined })
    .then(function(r) { return r.json().catch(function() { return {} }).then(function(j) { if (!r.ok) throw new Error(j.error || "HTTP " + r.status); return j }) })
}

function standalone() {
  var w = root
  return !!((w.navigator && w.navigator.standalone) || (w.matchMedia && w.matchMedia("(display-mode: standalone)").matches))
}

// Push to this phone, through the app's /api/push* (the kit crate's push::Push::route).
//   Kit.push.state()   "unsupported" | "install" (iOS: add it to the Home Screen first) |
//                      "denied" | "off" | "on"
//   Kit.push.enable()  ask (call it from a tap: iOS asks only then), subscribe, tell the
//                      server, and send a test notification; resolves to the new state
//   Kit.push.disable() unsubscribe here and there
//   Kit.push.refresh() tell the server this phone's subscription again, quietly (the button
//                      does it on load: a server that lost it, or keeps whose phone it is, has it)
/** @type {KitApi["push"]} */
var push = {
  state: function() {
    var nav = root.navigator
    if (!nav || !("serviceWorker" in nav)) return Promise.resolve("unsupported")
    if (!("PushManager" in root) || !("Notification" in root)) {
      return Promise.resolve(/iPhone|iPad/.test(nav.userAgent || "") && !standalone() ? "install" : "unsupported")
    }
    if (root.Notification.permission === "denied") return Promise.resolve("denied")
    return nav.serviceWorker.ready
      .then(function(reg) { return reg.pushManager.getSubscription() })
      .then(function(s) { return s ? "on" : "off" })
  },
  enable: function() {
    var nav = root.navigator, sub
    return root.Notification.requestPermission().then(function(p) {
      if (p !== "granted") return "denied"
      return Promise.all([nav.serviceWorker.ready, api("GET", "/api/push")]).then(function(x) {
        return x[0].pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: b64urlBytes(x[1].key) })
      }).then(function(s) {
        sub = s.toJSON()
        return api("POST", "/api/push/subscribe", sub)
      }).then(function() {
        return api("POST", "/api/push/test", { endpoint: sub.endpoint }).catch(function() {})
      }).then(function() { return "on" })
    })
  },
  refresh: function() {
    return root.navigator.serviceWorker.ready
      .then(function(reg) { return reg.pushManager.getSubscription() })
      .then(function(s) { return s ? api("POST", "/api/push/subscribe", s.toJSON()).then(function() { return true }) : false })
      .catch(function() { return false })
  },
  disable: function() {
    return root.navigator.serviceWorker.ready
      .then(function(reg) { return reg.pushManager.getSubscription() })
      .then(function(s) {
        if (!s) return "off"
        var endpoint = s.endpoint
        return s.unsubscribe().then(function() { return api("POST", "/api/push/unsubscribe", { endpoint: endpoint }).catch(function() {}) }).then(function() { return "off" })
      })
  }
}

var BELL = '<svg viewBox="0 0 24 24"><path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/></svg>'
var BELL_OFF = '<svg viewBox="0 0 24 24"><path d="M8.7 3A6 6 0 0 1 18 8a21.3 21.3 0 0 0 .6 5"/><path d="M17 17H3s3-2 3-9a4.67 4.67 0 0 1 .3-1.7"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/><path d="m2 2 20 20"/></svg>'
var PUSH_TITLES = { on: "Notifications on: tap to turn off", off: "Turn notifications on", denied: "Notifications are blocked in Settings", install: "Add to Home Screen for notifications", unsupported: "" }

// A header button (.btn.icon) that shows push's state and turns it on or off; null where
// push can't work at all (a desktop window: notify-send has that)
function pushButton() {
  var b = h("button", { "class": "btn icon kit-push", type: "button", hidden: true })
  function show(st) {
    b.hidden = st === "unsupported"
    b.dataset.state = st
    b.title = PUSH_TITLES[st] || ""
    b.setAttribute("aria-label", b.title)
    b.innerHTML = st === "on" ? BELL : BELL_OFF
  }
  b.addEventListener("click", function() {
    var st = b.dataset.state
    if (st === "install" || st === "denied") { alert(b.title); return }
    b.disabled = true
    ;(st === "on" ? push.disable() : push.enable())
      .then(function(s) { tick(); show(s) }, function(e) { alert("Notifications: " + e.message); return push.state().then(show) })
      .then(function() { b.disabled = false })
  })
  push.state().then(function(st) { show(st); if (st === "on") push.refresh() })
  return b
}

// The number on the app's icon (0 or null clears it); quietly nothing where there's no badge
function badge(n) {
  var nav = root.navigator
  if (!nav || !nav.setAppBadge) return Promise.resolve(false)
  return (n > 0 ? nav.setAppBadge(n) : nav.clearAppBadge()).then(function() { return true }, function() { return false })
}

// A haptic tick. iOS has no navigator.vibrate, but Safari (18+) ticks when a switch
// (<input type=checkbox switch>) is toggled, from a tap's handler; elsewhere vibrate(10)
/** @type {HTMLLabelElement | null} */
var ticker = null
function tick() {
  var nav = root.navigator, doc = root.document
  if (nav && nav.vibrate && !/iPhone|iPad/.test(nav.userAgent || "")) { try { nav.vibrate(10) } catch (e) {} return }
  if (!doc || !doc.body) return
  if (!ticker) {
    ticker = doc.createElement("label")
    ticker.className = "kit-tick"
    ticker.setAttribute("aria-hidden", "true")
    var i = doc.createElement("input")
    i.type = "checkbox"; i.setAttribute("switch", ""); i.tabIndex = -1
    ticker.appendChild(i)
    doc.body.appendChild(ticker)
  }
  // the click mustn't take focus (and the keyboard) from what had it
  var had = /** @type {HTMLElement | null} */ (doc.activeElement)
  ticker.click()
  if (had && had !== doc.activeElement && had.focus) had.focus({ preventScroll: true })
}

// The phone's share sheet for {title, text, url, files}; without one (a desktop window),
// the text and url go to the clipboard. Resolves to "shared", "copied" or "cancelled"
/** @type {KitApi["share"]} */
function share(data) {
  var nav = root.navigator
  if (nav && nav.share && (!data.files || !nav.canShare || nav.canShare(data))) {
    return nav.share(data).then(function() { return "shared" }, function() { return "cancelled" })
  }
  var text = [data.title, data.text, data.url].filter(Boolean).join("\n")
  if (nav && nav.clipboard) return nav.clipboard.writeText(text).then(function() { return "copied" }, function() { return "cancelled" })
  return Promise.resolve("cancelled")
}

// Change the page with a View Transition where there is one (Safari 18+, Chromium) and
// motion isn't reduced; a view-transition-name on both sides morphs one into the other
function transition(update) {
  var doc = root.document
  var still = root.matchMedia && root.matchMedia("(prefers-reduced-motion: reduce)").matches
  if (!doc.startViewTransition || still) { update(); return Promise.resolve() }
  return doc.startViewTransition(update).finished.catch(function() {})
}

// The on-screen keyboard's height in --kb (0px without one) and html.kit-kb while it's up:
// .kit-dock rides on it. Safari keeps the layout viewport under the keyboard, so a fixed
// bottom bar would be hidden behind it otherwise
function keyboard() {
  var vv = root.visualViewport, de = root.document.documentElement
  if (!vv) return
  function fit() {
    var v = /** @type {VisualViewport} */ (vv) // checked above; a closure doesn't see it
    var kb = Math.max(0, Math.round(root.innerHeight - v.height - v.offsetTop))
    de.style.setProperty("--kb", kb + "px")
    de.classList.toggle("kit-kb", kb > 80)
  }
  vv.addEventListener("resize", fit)
  vv.addEventListener("scroll", fit)
  fit()
}

// Swipe a row sideways for its actions: opts.right {label, run} for a swipe to the right,
// opts.left for one to the left (each optional; color: a CSS colour, else the accent). Past
// SWIPE_AT px it ticks and, let go, runs; touch only. The row is left as it is otherwise
var SWIPE_AT = 80
function swipeTravel(dx, width) {
  var free = Math.min(width * 0.45, 160)
  var a = Math.abs(dx), s = dx < 0 ? -1 : 1
  return s * (a <= free ? a : free + (a - free) * 0.3)
}
function swipe(row, opts) {
  var w = root
  if (!("ontouchstart" in w)) return row
  row.classList.add("kit-swipe")
  var under = row.querySelector(":scope > .kit-swipe-under")
  if (!under) { under = w.document.createElement("div"); under.className = "kit-swipe-under"; row.insertBefore(under, row.firstChild) }
  var x0 = 0, y0 = 0, dx = 0, armed = false
  /** @type {boolean | null} */
  var owned = null // null: not yet decided whether this touch is a swipe
  function side() { return dx > 0 ? opts.right : dx < 0 ? opts.left : null }
  function paint(animate) {
    row.classList.toggle("kit-swiping", !animate)
    row.style.setProperty("--kit-dx", dx + "px")
    var s = side()
    under.textContent = s ? s.label : ""
    under.dataset.side = dx > 0 ? "right" : "left"
    under.style.background = s ? (s.color || "var(--accent)") : ""
    row.classList.toggle("kit-swipe-armed", armed)
  }
  row.addEventListener("touchstart", function(e) {
    if (e.touches.length !== 1) return
    x0 = e.touches[0].clientX; y0 = e.touches[0].clientY; owned = null; dx = 0; armed = false
  }, { passive: true })
  row.addEventListener("touchmove", function(e) {
    var mx = e.touches[0].clientX - x0, my = e.touches[0].clientY - y0
    if (owned === null) {
      if (Math.abs(mx) < 8 && Math.abs(my) < 8) return
      owned = Math.abs(mx) > Math.abs(my) * 1.3 && !!(mx > 0 ? opts.right : opts.left)
    }
    if (!owned) return
    if (e.cancelable) e.preventDefault()
    dx = swipeTravel(mx, row.clientWidth)
    if (!(dx > 0 ? opts.right : opts.left)) dx = 0
    var nowArmed = Math.abs(dx) >= SWIPE_AT
    if (nowArmed !== armed) { armed = nowArmed; if (armed) tick() }
    paint(false)
  }, { passive: false })
  function end() {
    if (!owned) return
    var s = armed ? side() : null
    owned = null; armed = false; dx = 0
    paint(true)
    if (s) setTimeout(function() { s.run(row) }, 0)
  }
  row.addEventListener("touchend", end)
  row.addEventListener("touchcancel", function() { owned = null; armed = false; dx = 0; paint(true) })
  return row
}

// A card to fling sideways, one at a time (a deck: what's next comes in behind it): opts.right
// {label, color, run} for a fling to the right, opts.left for one to the left (each optional).
// It follows the pointer (touch or mouse; never from a link, button or field in it), tilts,
// and shows the side's label as a stamp; let go past FLING_AT px, or flicked faster than
// 0.5 px/ms, it flies off and then runs. go(side) flings it from a key or a button
var FLING_AT = 100
function flingTilt(dx, width) {
  var t = dx / Math.max(width, 1) * 18
  return Math.max(-20, Math.min(20, t))
}
function fling(card, opts) {
  card.classList.add("kit-fling")
  /** @type {Object<string, HTMLElement>} */
  var stamps = {}
  ;["left", "right"].forEach(function(side) {
    var o = opts[side]
    if (!o) return
    var st = h("div", { "class": "kit-fling-stamp", "data-side": side, style: o.color ? "color:" + o.color : null }, o.label)
    card.appendChild(st)
    stamps[side] = st
  })
  var x0 = 0, y0 = 0, t0 = 0, dx = 0, id = -1, gone = false, armed = false
  /** @type {boolean | null} */
  var owned = null
  function paint(dragging) {
    card.classList.toggle("kit-flinging", dragging)
    card.style.transform = dx ? "translateX(" + dx + "px) rotate(" + flingTilt(dx, card.clientWidth) + "deg)" : ""
    var p = Math.min(1, Math.abs(dx) / FLING_AT)
    if (stamps.right) stamps.right.style.opacity = dx > 0 ? String(p) : "0"
    if (stamps.left) stamps.left.style.opacity = dx < 0 ? String(p) : "0"
  }
  function go(side) {
    var o = opts[side]
    if (!o || gone) return
    gone = true
    var away = (side === "right" ? 1 : -1) * ((root.innerWidth || 800) + card.clientWidth)
    dx = dx || (side === "right" ? 1 : -1)
    paint(false)
    if (stamps[side]) stamps[side].style.opacity = "1"
    card.style.transform = "translateX(" + away + "px) rotate(" + (side === "right" ? 24 : -24) + "deg)"
    card.style.opacity = "0"
    var reduced = root.matchMedia && root.matchMedia("(prefers-reduced-motion: reduce)").matches
    setTimeout(function() { o.run(card) }, reduced ? 0 : 260)
  }
  card.addEventListener("pointerdown", function(e) {
    if (gone || id !== -1 || e.button > 0) return
    var t = /** @type {Element} */ (e.target)
    if (t.closest && t.closest("a, button, input, select, textarea")) return
    id = e.pointerId; x0 = e.clientX; y0 = e.clientY; t0 = e.timeStamp; dx = 0; owned = null; armed = false
  })
  card.addEventListener("pointermove", function(e) {
    if (e.pointerId !== id) return
    var mx = e.clientX - x0, my = e.clientY - y0
    if (owned === null) {
      if (Math.abs(mx) < 8 && Math.abs(my) < 8) return
      owned = Math.abs(mx) > Math.abs(my)
      if (!owned) { id = -1; return }
      try { card.setPointerCapture(id) } catch (_) {}
      x0 = e.clientX; t0 = e.timeStamp; mx = 0
    }
    dx = mx
    if (!(dx > 0 ? opts.right : opts.left)) dx = dx * 0.15
    var nowArmed = Math.abs(dx) >= FLING_AT && !!(dx > 0 ? opts.right : opts.left)
    if (nowArmed !== armed) { armed = nowArmed; if (armed) tick() }
    paint(true)
  })
  function end(e, cancel) {
    if (e.pointerId !== id) return
    id = -1
    if (!owned) return
    owned = null
    var speed = Math.abs(dx) / Math.max(1, e.timeStamp - t0)
    var side = dx > 0 ? "right" : "left"
    if (!cancel && opts[side] && (Math.abs(dx) >= FLING_AT || speed > 0.5 && Math.abs(dx) > 30)) return go(side)
    dx = 0; armed = false
    paint(false)
  }
  card.addEventListener("pointerup", function(e) { end(e, false) })
  card.addEventListener("pointercancel", function(e) { end(e, true) })
  // a drag that started on the card isn't a text selection or an image drag
  card.addEventListener("dragstart", function(e) { e.preventDefault() })
  return { go: go }
}

// A row of chips, one picked (a due date, a section): {options: [{value, label, hint?}], value,
// onChange(value)}. Picking the picked one again is a no-op; set(value) picks from outside.
// kit.css's .kit-choice draws it
function choice(o) {
  var box = h("div", { "class": "kit-choice", role: "group" })
  var cur = o.value
  function set(v) {
    cur = v
    Array.prototype.forEach.call(box.children, function(b) { b.setAttribute("aria-pressed", String(b.dataset.value === String(v))) })
  }
  ;(o.options || []).forEach(function(op) {
    var b = h("button", { type: "button", "data-value": String(op.value), onclick: function() {
      if (String(cur) === String(op.value)) return
      set(op.value); if (o.onChange) o.onChange(op.value)
    } }, [op.label, op.hint ? h("small", null, op.hint) : null])
    box.appendChild(b)
  })
  set(cur)
  return { el: box, set: set, get value() { return cur } }
}

// A textarea that grows with what's in it, up to max px (320), then scrolls; returns the fit
// function, for a value set from code
function autogrow(ta, max) {
  var top = max || 320
  function fit() {
    ta.style.height = "auto"
    var want = ta.scrollHeight + 2
    ta.style.height = Math.min(want, top) + "px"
    ta.style.overflowY = want > top ? "auto" : "hidden"
  }
  ta.addEventListener("input", fit)
  ;(root.requestAnimationFrame || setTimeout)(fit)
  return fit
}

// A bottom sheet of actions, in thumb reach: {title, text, actions: [{label, danger, run}]}.
// Resolves to the action's label (after its run), or null when dismissed
// Drag a bottom sheet down to close it, as on iOS: `box` follows the finger down, from
// anywhere on it while its content is scrolled to the top, and let go past a quarter of its
// height (or flicked down) it carries on out and `close` runs; else it springs back. Up, or
// in content scrolled down, stays the browser's scroll
var SHEET_FLICK = 0.5 // px/ms
function sheetDrag(box, close) {
  /** @type {number | null} */
  var y0 = null
  var x0 = 0, t0 = 0, dy = 0, owned = false
  function back() { box.style.transition = "transform .2s ease"; box.style.transform = "" }
  box.addEventListener("touchstart", function(e) {
    y0 = null
    if (e.touches.length !== 1 || box.scrollTop > 0) return
    y0 = e.touches[0].clientY; x0 = e.touches[0].clientX; t0 = Date.now(); dy = 0; owned = false
  }, { passive: true })
  box.addEventListener("touchmove", function(e) {
    if (y0 == null) return
    var d = e.touches[0].clientY - y0, dx = e.touches[0].clientX - x0
    if (!owned) {
      if (Math.abs(d) < 6 && Math.abs(dx) < 6) return
      // up or sideways first: the content's scroll, not ours
      if (d <= 0 || Math.abs(dx) >= d) { y0 = null; return }
      owned = true
      box.style.transition = "none"
    }
    if (e.cancelable) e.preventDefault() // no rubber band under the finger
    dy = Math.max(0, d)
    box.style.transform = dy ? "translateY(" + dy + "px)" : ""
  }, { passive: false })
  box.addEventListener("touchend", function() {
    if (y0 == null || !owned) { y0 = null; return }
    y0 = null
    var fast = dy / Math.max(1, Date.now() - t0) > SHEET_FLICK
    if (dy > box.offsetHeight / 4 || (fast && dy > 30)) {
      box.style.transition = "transform .18s ease"
      box.style.transform = "translateY(100%)"
      close()
    } else back()
  })
  box.addEventListener("touchcancel", function() { if (owned) back(); y0 = null })
}

// Show a bottom sheet (`dlg`, a dialog.kit-sheet around a .kit-sheet-box): it rises from
// below the screen once it's been drawn there. Started at once, the first frames went on
// laying it out and it showed up already halfway (a rise of 0.2 s is half done by 50 ms)
var SHEET_RISE = "transform .38s cubic-bezier(.32,.72,0,1)" // iOS's sheet curve
function sheetOpen(dlg) {
  var box = /** @type {HTMLElement | null} */ (dlg.querySelector(".kit-sheet-box"))
  var still = root.matchMedia && root.matchMedia("(prefers-reduced-motion: reduce)").matches
  if (box && !still) dlg.classList.add("kit-rising")
  dlg.showModal()
  if (!box || still) return
  var el = box
  root.requestAnimationFrame(function() {
    root.requestAnimationFrame(function() {
      el.style.transition = SHEET_RISE
      dlg.classList.remove("kit-rising")
      setTimeout(function() { if (el.style.transition === SHEET_RISE) el.style.transition = "" }, 420)
    })
  })
}

function sheet(o) {
  var doc = root.document
  return new Promise(function(done) {
    var dlg = h("dialog", { "class": "kit-sheet" })
    var settled = false
    function close(label) {
      if (settled) return
      settled = true
      dlg.classList.add("kit-closing")
      setTimeout(function() { if (dlg.open) dlg.close(); dlg.remove() }, 180)
      done(label)
    }
    var box = h("div", { "class": "kit-sheet-box" }, [
      h("div", { "class": "kit-sheet-grip", "aria-hidden": "true" }),
      o.title ? h("div", { "class": "kit-sheet-title" }, o.title) : null,
      o.text ? h("div", { "class": "kit-sheet-text" }, o.text) : null,
      (o.actions || []).map(function(a) {
        return h("button", { "class": "kit-sheet-act" + (a.danger ? " danger" : ""), type: "button", onclick: function() {
          tick()
          Promise.resolve(a.run && a.run()).then(function() { close(a.label) })
        } }, a.label)
      }),
      h("button", { "class": "kit-sheet-act cancel", type: "button", onclick: function() { close(null) } }, "Cancel")
    ])
    dlg.appendChild(box)
    sheetDrag(box, function() { close(null) })
    dlg.addEventListener("click", function(e) { if (e.target === dlg) close(null) })
    dlg.addEventListener("cancel", function(e) { e.preventDefault(); close(null) })
    doc.body.appendChild(dlg)
    sheetOpen(dlg)
  })
}

// Blur the page while the app is in the background, so the app switcher's picture of it
// shows nothing (net worth, a message). Best effort: iOS may take the picture first
function privacy() {
  var doc = root.document
  doc.addEventListener("visibilitychange", function() { doc.documentElement.classList.toggle("kit-veil", doc.visibilityState === "hidden") })
  root.addEventListener("pagehide", function() { doc.documentElement.classList.add("kit-veil") })
  root.addEventListener("pageshow", function() { doc.documentElement.classList.remove("kit-veil") })
}

// Face ID (Touch ID, the device's passcode) before the page shows, and again after `after`
// minutes away (default 5): a curtain against whoever holds the unlocked phone, not
// security (the tailnet is that). A WebAuthn passkey on this device that the page only
// asks for; nothing is sent anywhere. Where there's no platform authenticator (a desktop
// window), no curtain. opts: {app, after}
function lock(opts) {
  opts = opts || {}
  var w = root, doc = w.document, nav = w.navigator, key = "kit-lock:" + (opts.app || doc.title)
  var after = (opts.after == null ? 5 : opts.after) * 60000
  /** @type {number | null} */
  var hiddenAt = null
  /** @type {HTMLElement | null} */
  var veil = null
  if (!w.PublicKeyCredential || !nav.credentials || !w.PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable) return Promise.resolve(false)
  function rand(n) { var a = new Uint8Array(n); w.crypto.getRandomValues(a); return a }
  function b64(buf) { var s = "", a = new Uint8Array(buf); for (var i = 0; i < a.length; i++) s += String.fromCharCode(a[i]); return btoa(s) }
  function unb64(s) { var b = atob(s), a = new Uint8Array(b.length); for (var i = 0; i < b.length; i++) a[i] = b.charCodeAt(i); return a }
  function verify() {
    var id = w.localStorage.getItem(key)
    /** @type {PublicKeyCredentialRequestOptions} */
    var pk = { challenge: rand(32), userVerification: "required", timeout: 60000 }
    if (id) {
      pk.allowCredentials = [{ type: "public-key", id: unb64(id) }]
      return nav.credentials.get({ publicKey: pk })
    }
    return nav.credentials.create({ publicKey: {
      challenge: pk.challenge, rp: { name: opts.app || doc.title },
      user: { id: rand(16), name: opts.app || doc.title, displayName: opts.app || doc.title },
      pubKeyCredParams: [{ type: "public-key", alg: -7 }, { type: "public-key", alg: -257 }],
      authenticatorSelection: { authenticatorAttachment: "platform", userVerification: "required", residentKey: "discouraged" },
      timeout: 60000
    } }).then(function(c) { w.localStorage.setItem(key, b64(/** @type {PublicKeyCredential} */ (c).rawId)); return c })
  }
  function show() {
    if (veil) return
    doc.documentElement.classList.add("kit-locked")
    var first = !w.localStorage.getItem(key)
    var go = h("button", { "class": "btn primary", type: "button", onclick: function() {
      go.disabled = true
      verify().then(function() { hide() }, function() { go.disabled = false })
    } }, first ? "Set up Face ID" : "Unlock")
    var curtain = h("div", { "class": "kit-lock", role: "dialog", "aria-label": "Locked" }, [h("div", { "class": "kit-lock-name" }, opts.app || doc.title), go])
    veil = curtain
    doc.body.appendChild(curtain)
  }
  function hide() { if (veil) { veil.remove(); veil = null } doc.documentElement.classList.remove("kit-locked") }
  return w.PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable().then(function(ok) {
    if (!ok || !(nav.maxTouchPoints > 0)) return false
    show()
    doc.addEventListener("visibilitychange", function() {
      if (doc.visibilityState === "hidden") hiddenAt = Date.now()
      else if (hiddenAt != null && Date.now() - hiddenAt >= after) show()
    })
    return true
  }, function() { return false })
}

root.Kit = {
  pullToRefresh: pullToRefresh, pullTravel: pullTravel, PULL_AT: PULL_AT,
  loadTheme: loadTheme, applyTheme: applyTheme, themeVars: themeVars, goodColor: goodColor, warnColor: warnColor, urgentColor: urgentColor, onAccent: onAccent, readable: readable, contrastOf: contrastOf,
  rgbOf: rgbOf, hex: hex, mix: mix, luminance: luminance, contrast: contrast, isGreen: isGreen, isAmber: isAmber, h: h,
  offline: offline, fmtSaved: fmtSaved, autoUpdate: autoUpdate, updateNow: updateNow, sheetDrag: sheetDrag, sheetOpen: sheetOpen, pageChanged: pageChanged, push: push, pushButton: pushButton, badge: badge, tick: tick, share: share,
  transition: transition, keyboard: keyboard, swipe: swipe, swipeTravel: swipeTravel, SWIPE_AT: SWIPE_AT,
  fling: fling, flingTilt: flingTilt, FLING_AT: FLING_AT, sheet: sheet, choice: choice, autogrow: autogrow,
  privacy: privacy, lock: lock, standalone: standalone
}
})(typeof window !== "undefined" ? window : this)
