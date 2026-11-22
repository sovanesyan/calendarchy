// Calendarchy's phone page (served no-cache, so a change reaches the phone on its next load).
// One view, the day. /api/agenda is `calendarchy --agenda`'s JSON; every display rule is the shell window's own
// (/layout.js is shell/window/Layout.js, its functions global here, reached through L).
// Everything from the calendars goes in through Kit.h's text nodes, never HTML.
;(function() {
"use strict"

/** @type {any} */
var L = window
var h = Kit.h
var $ = function(id) { return /** @type {HTMLElement} */ (document.getElementById(id)) }

// The week before the selected one (a drag back from a Monday shows Sunday), the selected
// week and two after it (the strip, the day, and "next" two weeks ahead).
// Asked every minute with the last ETag, so an unchanged minute is a 304 with no body
var DAYS = 28

/** @typedef {{id: string, title: string, allDay: boolean, start: number|null, end: number|null, spansDays: boolean,
 *   accepted: boolean, free: boolean, busy: number[]|null, meetingUrl: string|null, joinUrl: string|null,
 *   location: string|null, description: string|null, calendarName: string|null,
 *   attendees: {name: string|null, email: string, status: string}[]}} Ev */

var S = {
  /** @type {any} */ agenda: null,
  /** @type {Record<string, any>} */ byDate: {},
  today: "", now: 0,
  selected: "",
  loading: false, refreshing: false,
  /** @type {string|null} */ error: null,
  /** @type {{text: string, bad?: boolean}|null} */ status: null,
  /** @type {Record<string, boolean>} */ refreshed: {},
  seq: 0,
  /** @type {string|null} */ scrolledFor: null,
  /** @type {Record<string, string>} */ etags: {}
}
var toastTimer = 0

function clock() {
  var d = new Date()
  S.today = L.fromDate(d)
  S.now = d.getHours() * 60 + d.getMinutes()
}

function twelve() { return !!(S.agenda && S.agenda.twelveHour) }
function sundayFirst() { return !!(S.agenda && S.agenda.sundayFirst) }
function workingHours() { return S.agenda ? S.agenda.workingHours : null }
function sources() {
  var list = (S.agenda && S.agenda.sources) || [{ key: "google", label: "Work", configured: true }, { key: "icloud", label: "Personal", configured: true }]
  var on = list.filter(function(s) { return s.configured })
  return on.length ? on : list
}
function label(key) {
  var s = sources().filter(function(x) { return x.key === key })[0]
  return s ? s.label : key
}

// ------------------------------------------------------------------ data

function api(path, opts) {
  return fetch(path, opts).then(function(r) {
    return r.json().catch(function() { return {} }).then(function(body) {
      if (!r.ok) throw new Error(body.error || ("HTTP " + r.status))
      return body
    })
  })
}

function post(path) {
  return api(path, { method: "POST", headers: { "X-Calendarchy": "1" } })
}

function rangeFrom() { return L.weekStart(S.selected, sundayFirst()) }

function load() {
  var seq = ++S.seq, from = rangeFrom(), url = "/api/agenda?from=" + L.addDays(from, -7) + "&days=" + DAYS
  S.loading = true
  /** @type {Record<string, string>} */
  var headers = {}
  if (S.etags[url] && S.agenda) headers["If-None-Match"] = S.etags[url]
  return fetch(url, { headers: headers }).then(function(r) {
    if (r.status === 304) return null
    return r.json().catch(function() { return {} }).then(function(body) {
      if (!r.ok) throw new Error(body.error || ("HTTP " + r.status))
      var tag = r.headers.get("ETag")
      if (tag) S.etags[url] = tag
      return body
    })
  }).then(function(a) {
    if (seq !== S.seq) return
    S.loading = false
    S.error = null
    if (!a) return render() // unchanged: just the clock moved
    var wasSunday = sundayFirst()
    S.agenda = a
    for (var i = 0; i < a.days.length; i++) S.byDate[a.days[i].date] = a.days[i]
    // the first answer says which day weeks start on: the grid may start elsewhere
    if (wasSunday !== sundayFirst() && rangeFrom() !== from) return load()
    render()
  }, function(e) {
    if (seq !== S.seq) return
    S.loading = false
    S.error = String(e.message || e)
    render()
  })
}

// Months the sync job doesn't keep fresh (it does this one and the next) are fetched once
// when you look at them, as the window does
function ensureMonth() {
  var month = L.firstOfMonth(S.selected), thisMonth = L.firstOfMonth(S.today)
  if (month === thisMonth || month === L.addMonths(thisMonth, 1) || S.refreshed[month]) return
  S.refreshed[month] = true
  refetch(month, true)
}

// Fetch a month from Google and iCloud into the cache, then read it again
function refetch(month, quiet) {
  if (S.refreshing) return Promise.resolve()
  S.refreshing = true
  header()
  return post("/api/refresh?month=" + month.slice(0, 7)).then(function() {
    if (!quiet) say("Up to date with Google and iCloud")
  }, function(e) {
    say(String(e.message || e), true)
  }).then(function() {
    S.refreshing = false
    header()
    return load()
  })
}

// A short message in a pill on the bottom edge
function say(text, bad) {
  var t = $("toast")
  t.textContent = text
  t.className = "toast on" + (bad ? " bad" : "")
  clearTimeout(toastTimer)
  toastTimer = setTimeout(function() { t.className = "toast" }, bad ? 6000 : 2500)
}

// ------------------------------------------------------------------ navigation

function select(date) {
  var week = rangeFrom(), month = L.firstOfMonth(S.selected)
  // choosing the day you're on (Today, mostly) scrolls back to now or its first event
  if (date === S.selected) S.scrolledFor = null
  S.selected = date
  try { history.replaceState(null, "", "#" + (date === S.today ? "" : date)) } catch (_) {}
  render()
  if (rangeFrom() !== week) load()
  if (L.firstOfMonth(date) !== month) ensureMonth()
}

function step(n) { go(L.addDays(S.selected, n)) }

// ------------------------------------------------------------------ moving between days

// The days are the browser's own sideways scroller (.days): SPAN days before the day and
// SPAN after, a screen each, snapping one at a time. So the browser moves them with the finger
// at the screen's own rate (120 Hz on a ProMotion iPhone) with its own momentum, where a
// transform set from touch events gets 60 at best and drops frames when the page is busy.
// Come to rest on another day and that one is the day: they're drawn again around it, back in
// the middle, and nothing seen moves. That waits for the finger to lift and the scroll to stop,
// so SPAN is the room for quick swipes one after another before it does (with one a side, a
// second swipe that started while the first was settling hit the end and went nowhere)
var SPAN = 3, MID = SPAN
/** @type {string[]} */
var panes = []          // the dates drawn in the panes, MID the selected one
var touching = false    // a finger is on the days
var settleTimer = 0     // scroll events still coming
var renderLater = false // a redraw that waited for the days to stop moving
var going = ""          // the day a tap or a key is scrolling to

function stillMotion() { return window.matchMedia("(prefers-reduced-motion: reduce)").matches }
function moving() { return touching || !!settleTimer }
function sides() { return /** @type {HTMLElement[]} */ (Array.prototype.slice.call(document.querySelectorAll("#days > .pane > main"))) }

// Go to `date`: scrolled to where it's drawn, or first drawn in the pane next to the day on
// the side time goes (later on the right)
function go(date) {
  if (date === S.selected || !panes.length || !S.agenda || stillMotion()) { select(date); return }
  var k = panes.indexOf(date), days = $("days")
  if (k < 0) {
    k = MID + (date > S.selected ? 1 : -1)
    var pane = sides()[k]
    pane.textContent = ""
    day(pane, date, true)
    panes[k] = date
    pillSpan()
  }
  going = date
  days.scrollTo({ left: days.clientWidth * k, behavior: "smooth" })
}

function scrolled() {
  pillFollow()
  clearTimeout(settleTimer)
  settleTimer = setTimeout(settled, 120)
}

function settled() {
  clearTimeout(settleTimer)
  settleTimer = 0
  if (touching) return
  var days = $("days"), w = days.clientWidth, i = Math.round(days.scrollLeft / w)
  if (Math.abs(days.scrollLeft - i * w) > 2) return // still on its way to a day
  var date = panes[i]
  if (i === MID || !date) { if (renderLater) render(); return }
  // swiped there: the same time of day (a tap opens it at now or its first event)
  if (going !== date) { Kit.tick(); S.scrolledFor = date }
  going = ""
  select(date)
}

// The middle pane in view, and the days as tall as the day (at least the screen), so a
// taller neighbour doesn't leave the page scrolling on past an empty day
function centre() {
  var days = $("days"), head = document.querySelector("header")
  days.style.height = Math.max($("main").offsetHeight, window.innerHeight - (head ? head.offsetHeight : 0)) + "px"
  days.scrollLeft = days.clientWidth * MID
  pillFollow()
}

// ------------------------------------------------------------------ drawing

function render() {
  header()
  // a swipe is under way: the days wait for it to come to rest
  if (moving()) { renderLater = true; return }
  renderLater = false
  var all = sides(), main = $("main")
  all.forEach(function(el) { el.textContent = "" })
  panes = all.map(function(_, k) { return L.addDays(S.selected, k - MID) })
  pillSpan()
  if (!S.agenda) {
    main.appendChild(S.error ? h("div", { "class": "err" }, "Couldn't load the calendar: " + S.error) : h("div", { "class": "empty" }, "Loading…"))
    return centre()
  }
  if (S.error) main.appendChild(h("div", { "class": "msg bad" }, S.error))
  all.forEach(function(el, k) { if (k === MID) day(el); else day(el, panes[k], true) })
  centre()
}

var MONTHS = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"]
var WEEKDAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"]

// The sticky bar: the day, Today, refetch, and the week strip
function header() {
  var sel = S.selected
  if (!sel) return
  var d = L.toDate(sel), isToday = sel === S.today
  var title = $("title")
  title.textContent = ""
  title.appendChild(h("b", null, WEEKDAYS[d.getDay()] + " " + d.getDate() + " " + MONTHS[d.getMonth()]))
  title.appendChild(h("small", null, S.refreshing ? "Refreshing…" : isToday ? "Today" : relative(sel)))
  $("today").hidden = isToday
  $("refresh").classList.toggle("spin", S.refreshing)

  var first = L.weekStart(sel, sundayFirst()), letters = L.weekdayLetters(sundayFirst())
  var week = $("week"), copies = []
  week.textContent = ""
  for (var i = 0; i < 7; i++) {
    var date = L.addDays(first, i)
    var face = function() { return [h("small", null, letters[i]), h("b", null, String(Number(date.slice(8)))), heavyBar(date)] }
    week.appendChild(h("button", {
      "class": "wd" + (date === S.today ? " today" : "") + (date === sel ? " sel" : "") + (L.isWeekend(date) ? " weekend" : ""),
      type: "button", "aria-label": L.dayTitle(date), "aria-pressed": String(date === sel), onclick: go.bind(null, date)
    }, face()))
    copies.push(h("span", { "class": "wd" }, face()))
  }
  // the selected day's pill, over the strip: it moves with the days as they scroll (one day's
  // width for a day's scroll), and the copy of the strip inside it, in the pill's colours,
  // moves back the same, so the days under it read inverted wherever it is
  var at = (L.toDate(sel).getDay() - (sundayFirst() ? 0 : 1) + 7) % 7
  week.appendChild(h("div", { "class": "selpill", id: "selpill", "aria-hidden": "true", style: "--i:" + at },
    h("div", { "class": "in" }, copies)))
  pillSpan()
}

// Where the pill is with each pane in view, in days from the selected one (--o0…): k - MID,
// but for a tapped day drawn next to it (go)
function pillSpan() {
  var pill = $("selpill")
  if (!pill || !panes.length) return
  panes.forEach(function(d, k) { pill.style.setProperty("--o" + k, String(dayGap(S.selected, d))) })
}
function dayGap(a, b) { return Math.round((L.toDate(b).getTime() - L.toDate(a).getTime()) / 86400000) }

// Without scroll-driven animations (an older Safari) the pill follows from the scroll events
var CSS_PILL = !!(window.CSS && CSS.supports && CSS.supports("animation-timeline: --days"))
function pillFollow() {
  var pill = $("selpill"), days = $("days")
  if (CSS_PILL || !pill || !panes.length) return
  var at = Math.max(0, Math.min(panes.length - 1, days.scrollLeft / (days.clientWidth || 1)))
  var k = Math.min(panes.length - 2, Math.floor(at)), f = at - k
  var off = function(j) { return Number(pill.style.getPropertyValue("--o" + j)) || 0 }
  var n = off(k) + (off(k + 1) - off(k)) * f
  pill.style.transform = n ? "translateX(calc(" + n + " * (100% + 2px)))" : ""
  var inner = /** @type {HTMLElement} */ (pill.firstChild)
  inner.style.transform = n ? "translateX(calc(" + (-n) + " * ((100% - 12px) / 7 + 2px)))" : ""
}

function heavyBar(date) {
  var busy = L.busyMinutes(L.dayOf(S.byDate, date))
  var pct = Math.min(100, Math.round(busy / L.HEAVY_DAY_MINUTES * 100))
  return h("i", null, busy ? h("span", { "class": busy >= L.HEAVY_DAY_MINUTES ? "heavy" : "", style: "width:" + Math.max(pct, 8) + "%" }) : null)
}

// ---- Day

// The day `date` (the selected one by default) into `main`. A ghost is a neighbour in a side
// pane: the same, but it never scrolls the page or marks the day seen
function day(main, date, ghost) {
  var sel = date || S.selected, isToday = sel === S.today
  var next = S.agenda.next
  if (isToday && next) main.appendChild(nextCard(next))

  var m = merge(L.dayOf(S.byDate, sel))
  var allDay = m.rows.filter(function(r) { return r.e.allDay })
  var timed = m.rows.filter(function(r) { return !r.e.allDay })
  if (allDay.length) main.appendChild(h("div", { "class": "chips" }, allDay.map(function(r) {
    var srcs = sourcesOf(r)
    return h("button", { "class": "chip " + r.source + (srcs.length > 1 ? " both" : "") + (r.e.accepted ? "" : " unaccepted"), type: "button",
      onclick: function() { details(r.e, sel, r.source, r.also) } }, [srcs.length > 1 ? dots(srcs) : null, r.e.title || "(no title)", r.e.spansDays ? h("small", null, " · several days") : null])
  })))
  // an empty day is a line, not an empty grid
  if (!timed.length) {
    var nu = nextUp(sel)
    if (!ghost) S.scrolledFor = sel
    main.appendChild(h("div", { "class": "none" }, [allDay.length ? "Nothing at a set time" : "Nothing scheduled", nu ? h("small", null, "Next: " + nu) : null]))
    return
  }
  main.appendChild(timeline(m, timed, sel, isToday, ghost))
}

// ---- The day as a timeline: hours down the side, events as blocks by their times

var HOUR_PX = 72 // a half hour is 36 px: a tap target, not a sliver

function blockEnd(e) { return Math.min(L.DAY_MINUTES, Math.max(endOf(e), e.start + 15)) }

// Side by side where they overlap: each block's column and how many share its cluster
function columns(rows) {
  var sorted = rows.slice().sort(function(a, b) { return a.e.start - b.e.start || blockEnd(b.e) - blockEnd(a.e) })
  var out = [], cluster = [], ends = [], clusterEnd = -1
  function close() { cluster.forEach(function(c) { c.cols = ends.length }); cluster = []; ends = [] }
  sorted.forEach(function(r) {
    if (r.e.start >= clusterEnd) { close(); clusterEnd = -1 }
    var col = ends.findIndex(function(end) { return end <= r.e.start })
    if (col < 0) { col = ends.length; ends.push(0) }
    ends[col] = blockEnd(r.e)
    var item = { r: r, col: col, cols: 1 }
    cluster.push(item); out.push(item)
    clusterEnd = Math.max(clusterEnd, blockEnd(r.e))
  })
  close()
  return out
}

function timeline(m, rows, sel, isToday, ghost) {
  // 8:00 to 19:00 at least, stretched to the day's first and last events
  var first = 8 * 60, last = 19 * 60
  rows.forEach(function(r) { first = Math.min(first, r.e.start); last = Math.max(last, blockEnd(r.e)) })
  var from = Math.floor(first / 60) * 60, to = Math.min(L.DAY_MINUTES, Math.ceil(last / 60) * 60)
  var px = HOUR_PX / 60
  var y = function(min) { return Math.round((min - from) * px) }
  var tl = h("section", { "class": "tl", style: "height:" + y(to) + "px" })

  var wh = workingHours()
  if (wh) tl.appendChild(h("div", { "class": "work", style: "top:" + y(Math.max(wh[0], from)) + "px;height:" + Math.max(0, y(Math.min(wh[1], to)) - y(Math.max(wh[0], from))) + "px" }))
  for (var t = from; t <= to; t += 60) {
    tl.appendChild(h("div", { "class": "hr", style: "top:" + y(t) + "px" }, t < L.DAY_MINUTES ? h("span", null, hourLabel(t / 60)) : null))
    if (t < to) tl.appendChild(h("div", { "class": "hr half", style: "top:" + y(t + 30) + "px" }))
  }

  var area = h("div", { "class": "area" })
  // free gaps (the window's rule): the empty space says it, a quiet label says how long
  var g = L.gaps(m.day, sel, S.today, S.now, wh)
  g.google.concat(g.icloud).forEach(function(gp) {
    // at the top of the gap: an event that isn't busy (not accepted, marked free) can sit lower in it
    area.appendChild(h("div", { "class": "free" + (gp.now ? " now" : ""), style: "top:" + (y(gp.from) + 4) + "px" },
      (gp.now ? "free now · " : "free ") + L.formatDuration(gp.minutes)))
  })

  var cn = L.currentNext(rows.map(function(r) { return r.e }), isToday, S.now)
  columns(rows).forEach(function(c) {
    var r = c.r, e = r.e, i = rows.indexOf(r), srcs = sourcesOf(r)
    var top = y(e.start), hgt = Math.max(20, y(blockEnd(e)) - top - 2)
    var past = sel < S.today || (isToday && endOf(e) <= S.now)
    var url = e.meetingUrl || r.also.map(function(a) { return a.e.meetingUrl }).filter(Boolean)[0] || null
    var kind = L.meetingKind(url), len = L.formatLength(e)
    var cls = "blk " + r.source + (srcs.length > 1 ? " both" : "") + (past && i !== cn.current ? " past" : "") +
      (i === cn.current ? " current" : "") + (e.accepted ? "" : " unaccepted") + (e.free ? " freeev" : "") +
      (hgt < 30 ? " short" : "") + (c.cols >= 3 ? " narrow" : "")
    // the block's place and size say when; the time is only for a screen reader
    var when = L.timeRange(e, twelve()) + (len ? " · " + len : "") + (kind ? " · " + kind : "")
    area.appendChild(h("button", {
      "class": cls, type: "button",
      style: "top:" + top + "px;height:" + hgt + "px;left:calc(" + (c.col / c.cols * 100) + "% + 2px);width:calc(" + (100 / c.cols) + "% - 4px)",
      "aria-label": e.title + ", " + when, onclick: function() { details(e, sel, r.source, r.also) }
    }, h("span", { "class": "t" }, [i === cn.next ? h("span", { "class": "nx" }, "next") : null, srcs.length > 1 ? dots(srcs) : null, e.title || "(no title)"])))
  })
  tl.appendChild(area)

  if (isToday && S.now >= from && S.now <= to) tl.appendChild(h("div", { "class": "nowline", style: "top:" + y(S.now) + "px" }))

  // a new day opens at now (today) or its first event, not at 08:00 every time
  if (!ghost && S.scrolledFor !== sel) {
    S.scrolledFor = sel
    var at = isToday ? S.now - 60 : rows.length ? rows[0].e.start - 30 : 8 * 60
    requestAnimationFrame(function() {
      var head = document.querySelector("header")
      var top = tl.getBoundingClientRect().top + window.scrollY + y(Math.max(from, at)) - (head ? head.offsetHeight : 0) - 8
      window.scrollTo({ top: Math.max(0, top), behavior: "instant" })
    })
  }
  return tl
}

// "8" … "19", or "8 AM" with 12-hour time
function hourLabel(hr) {
  if (!twelve()) return (hr < 10 ? "0" : "") + hr + ":00"
  return (hr % 12 || 12) + (hr < 12 || hr === 24 ? " AM" : " PM")
}

function sourcesOf(r) { return [r.source].concat(r.also.map(function(a) { return a.source })) }
function dots(srcs) { return h("span", { "class": "srcs", title: srcs.map(label).join(" · ") }, srcs.map(function(k) { return h("i", { "class": k }) })) }

// ---- Both calendars as one day

// A title as compared for duplicates: case, punctuation and emoji don't count
function norm(t) { return (t || "").toLowerCase().replace(/[^\p{L}\p{N}]+/gu, " ").trim() }

// The same event in both calendars (an invite copied to the other, a shared calendar):
// same start and end (or both all day), and the same title, or one inside the other
function sameEvent(a, b) {
  if (a.allDay !== b.allDay || a.spansDays !== b.spansDays) return false
  if (!a.allDay && (a.start !== b.start || endOf(a) !== endOf(b))) return false
  var x = norm(a.title), y = norm(b.title)
  if (!x || !y) return false
  return x === y || (Math.min(x.length, y.length) >= 4 && (x.indexOf(y) >= 0 || y.indexOf(x) >= 0))
}

// A Work event that came from the Personal calendar: an invitation sent from iCloud (its
// organizer is iCloud's invitation relay, imip.me.com, or an iCloud address). It's Personal
var ICLOUD_ORGANIZER = /@(imip\.me\.com|icloud\.com|me\.com|mac\.com)$/i
function fromPersonal(e) {
  return e.attendees.some(function(a) { return a.status === "organizer" && ICLOUD_ORGANIZER.test(a.email) })
}

// Both calendars as one list: rows {e, source, also: [{e, source}]}, all day first, then by
// start (Work first on a tie). A Personal event that duplicates a Work one goes in its `also`
// (the Work copy leads: it has the invite, the people and the link), unless the Work copy is
// an invitation from the Personal calendar: then the Personal one stands alone, and so does
// such an invitation with no copy (drawn as Personal). `day` is the day with each duplicate
// once, for gaps and overlaps
function merge(dayData) {
  var rows = (dayData.google || []).map(function(e) { return { e: e, source: fromPersonal(e) ? "icloud" : "google", also: [], invite: fromPersonal(e) } })
  var kept = [], dropped = []
  ;(dayData.icloud || []).forEach(function(e) {
    var twin = rows.filter(function(r) { return r.e.id.indexOf("google:") === 0 && !r.also.length && sameEvent(r.e, e) })[0]
    if (twin && twin.invite) { dropped.push(twin.e); twin.e = e; twin.invite = false; kept.push(e) }
    else if (twin) twin.also.push({ e: e, source: "icloud" })
    else { rows.push({ e: e, source: "icloud", also: [] }); kept.push(e) }
  })
  var key = function(r) { return r.e.allDay ? -1 : r.e.start }
  rows.forEach(function(r, n) { r.n = n })
  rows.sort(function(a, b) { return key(a) - key(b) || a.n - b.n })
  var google = (dayData.google || []).filter(function(e) { return dropped.indexOf(e) < 0 })
  return { rows: rows, day: { date: dayData.date, google: google, icloud: kept } }
}

// "tmrw 09:00 Title": the first accepted timed event after `date` in either calendar, 14 days ahead
function nextUp(date) {
  for (var d = 1; d <= 14; d++) {
    var day = L.addDays(date, d), m = merge(L.dayOf(S.byDate, day))
    for (var i = 0; i < m.rows.length; i++) {
      var e = m.rows[i].e
      if (!e.accepted || e.allDay) continue
      var near = L.dayDiff(S.today, day) <= 6
      return (near ? L.smartWhen(day, e.start, S.today, twelve()) : L.shortDay(day) + " " + L.hm(e.start, twelve())) + " · " + e.title
    }
  }
  return ""
}

function endOf(e) { return e.end === null || e.end === undefined ? e.start + 60 : e.end }

function relative(date) {
  var n = L.dayDiff(S.today, date)
  if (n === 1) return "tomorrow"
  if (n === -1) return "yesterday"
  return n > 0 ? "in " + n + " days" : -n + " days ago"
}

function nextCard(next) {
  var f = L.footerNext(next, S.today, S.now)
  var when = next.current ? "until " + (next.end != null ? L.hm(next.end, twelve()) : "") : L.smartWhen(next.date, next.start, S.today, twelve()) + (f ? f.rest : "")
  var card = h("div", { "class": "upnext" + (next.current ? " now" : ""), role: "button", tabindex: "0", onclick: function() { details(next, next.date, sourceOf(next)) } }, [
    h("div", { "class": "grow" }, [
      h("div", { "class": "k" }, next.current ? "Now" : "Next"),
      h("div", { "class": "t" }, next.title),
      h("div", { "class": "w" }, when)
    ]),
    next.joinUrl ? h("a", { "class": "join", href: next.joinUrl, onclick: function(ev) { ev.stopPropagation(); Kit.tick() } }, "Join") : null
  ])
  return card
}

function sourceOf(e) { return e.id.indexOf("icloud:") === 0 || fromPersonal(e) ? "icloud" : "google" }

// ------------------------------------------------------------------ the event

function details(e, date, source, also) {
  also = also || []
  var dlg = h("dialog", { "class": "kit-sheet" })
  var closed = false
  function close() {
    if (closed) return
    closed = true
    dlg.classList.add("kit-closing")
    setTimeout(function() { if (dlg.open) dlg.close(); dlg.remove() }, 180)
  }
  var len = L.formatLength(e)
  var when = L.dayTitle(date) + " · " + L.timeRange(e, twelve()) + (len ? " (" + len + ")" : "")
  var calOf = function(ev, src) { return ev.calendarName && ev.calendarName !== label(src) ? label(src) + " · " + ev.calendarName : label(src) }
  var cal = [calOf(e, source)].concat(also.map(function(a) { return calOf(a.e, a.source) })).join(" + ")
  // a duplicate's own link when the leading copy has none
  if (!e.joinUrl) also.forEach(function(a) { if (!e.joinUrl && a.e.joinUrl) e = Object.assign({}, e, { joinUrl: a.e.joinUrl, meetingUrl: a.e.meetingUrl }) })

  var acts = h("div", { "class": "acts" })
  if (e.joinUrl) acts.appendChild(h("a", { "class": "join", href: e.joinUrl, onclick: function() { Kit.tick() } }, "Join " + (L.meetingKind(e.meetingUrl) === "link" ? "" : L.meetingKind(e.meetingUrl))))
  if (e.meetingUrl && e.meetingUrl !== e.joinUrl) acts.appendChild(h("a", { "class": "btn", href: e.meetingUrl, target: "_blank", rel: "noopener" }, "Open link"))
  if (source === "google" && e.attendees.length) {
    // the agenda knows whether you accepted (or organise it), not which attendee you are
    if (!e.accepted) acts.appendChild(h("button", { "class": "btn ok", type: "button", onclick: function() { act("accept", e, close) } }, "Accept"))
    acts.appendChild(h("button", { "class": "btn danger", type: "button", onclick: function() { act("decline", e, close) } }, "Decline"))
  }

  var body = [
    h("div", { "class": "kit-sheet-grip", "aria-hidden": "true" }),
    h("div", { "class": "dt" }, [
      h("div", { "class": "kit-sheet-title" }, e.title || "(no title)"),
      h("div", { "class": "when" }, [h("span", { "class": "srcs" }, [source].concat(also.map(function(a) { return a.source })).map(function(k) { return h("i", { "class": k }) })), when, " · ", cal]),
      also.length ? h("div", { "class": "same" }, "In both calendars: shown once") : null,
      !e.accepted ? h("div", { "class": "note" }, "Not accepted (declined, maybe or no reply yet)") : null,
      e.free ? h("div", { "class": "note" }, "Marked free: it doesn't block time") : null,
      acts.childNodes.length ? acts : null,
      e.location ? [h("div", { "class": "lab" }, "Where"), h("div", { "class": "txt" }, place(e.location))] : null,
      e.attendees.length ? attendees(e.attendees) : null,
      e.description ? [h("div", { "class": "lab" }, "Notes"), h("div", { "class": "txt" }, linkify(L.descriptionText(e.description)))] : null
    ]),
    h("button", { "class": "kit-sheet-act cancel", type: "button", onclick: close }, "Close")
  ]
  var box = h("div", { "class": "kit-sheet-box" }, body)
  dlg.appendChild(box)
  Kit.sheetDrag(box, close) // dragged down, it goes
  dlg.addEventListener("click", function(ev) { if (ev.target === dlg) close() })
  dlg.addEventListener("cancel", function(ev) { ev.preventDefault(); close() })
  document.body.appendChild(dlg)
  dlg.showModal()
}

function attendees(list) {
  var sorted = list.slice().sort(function(a, b) { return rank(a.status) - rank(b.status) || who(a).localeCompare(who(b)) })
  var rows = sorted.map(function(a) {
    var mark = { accepted: "✓", organizer: "★", declined: "✗", tentative: "?", needsAction: "·" }[a.status] || "·"
    return h("div", { "class": "who" }, [
      h("span", { "class": "st " + a.status, "aria-label": a.status }, mark),
      h("span", { "class": "nm" }, [who(a), a.name ? h("small", null, " " + a.email) : null])
    ])
  })
  var lab = h("div", { "class": "lab" }, "People · " + L.attendeeSummary(list))
  if (rows.length <= 8) return [lab].concat(rows)
  return [lab, rows.slice(0, 6), h("details", null, [h("summary", { "class": "who" }, (rows.length - 6) + " more"), rows.slice(6)])]
}
function rank(s) { return { organizer: 0, accepted: 1, tentative: 2, needsAction: 3, declined: 4 }[s] || 5 }
function who(a) { return a.name || a.email }

// A location that's a link opens it; anything else opens in Maps
function place(loc) {
  if (/^https?:\/\//i.test(loc)) return h("a", { href: loc, target: "_blank", rel: "noopener" }, loc)
  return h("a", { href: "https://maps.apple.com/?q=" + encodeURIComponent(loc), target: "_blank", rel: "noopener" }, loc)
}

// Plain text with its http(s) links made into links (text nodes and <a>s, never HTML)
function linkify(text) {
  var out = [], re = /https?:\/\/[^\s<>"')\]]+/g, last = 0, m
  while ((m = re.exec(text))) {
    if (m.index > last) out.push(text.slice(last, m.index))
    out.push(h("a", { href: m[0], target: "_blank", rel: "noopener" }, m[0]))
    last = m.index + m[0].length
  }
  if (last < text.length) out.push(text.slice(last))
  return out
}

function act(verb, e, closeDetails) {
  var v = L.VERBS[verb]
  Kit.sheet({ title: v.prompt, text: e.title, actions: [{ label: v.name, danger: verb === "decline" }] }).then(function(chosen) {
    if (!chosen) return
    closeDetails()
    say(v.doing)
    post("/api/event?verb=" + verb + "&id=" + encodeURIComponent(e.id)).then(function() {
      say(v.done)
      return load()
    }, function(err) { say(String(err.message || err), true) })
  })
}

// ------------------------------------------------------------------ wiring

// The days follow the finger by themselves (.days scrolls); this only knows when a finger is on
// them, so nothing redraws under it
function holdDays() {
  var days = $("days")
  days.addEventListener("scroll", scrolled, { passive: true })
  if ("onscrollend" in window) days.addEventListener("scrollend", settled)
  days.addEventListener("touchstart", function() { touching = true }, { passive: true })
  var up = function() { touching = false; scrolled() }
  days.addEventListener("touchend", up, { passive: true })
  days.addEventListener("touchcancel", up, { passive: true })
  if (window.ResizeObserver) new ResizeObserver(function() { if (!moving()) centre() }).observe($("main"))
  window.addEventListener("resize", function() { if (!moving()) centre() })
}

// A flick sideways on the week strip moves a week
function flick(el, by) {
  var x0 = 0, y0 = 0, t0 = 0, dx = 0, on = false, axis = ""
  el.addEventListener("touchstart", function(ev) {
    if (ev.touches.length !== 1 || document.querySelector("dialog[open]")) { on = false; return }
    on = true; axis = ""; dx = 0
    x0 = ev.touches[0].clientX; y0 = ev.touches[0].clientY; t0 = Date.now()
  }, { passive: true })
  el.addEventListener("touchmove", function(ev) {
    if (!on) return
    var t = ev.touches[0], mx = t.clientX - x0, my = t.clientY - y0
    // the first few pixels decide: a scroll up or down is left alone
    if (!axis && Math.abs(mx) + Math.abs(my) > 10) axis = Math.abs(mx) > Math.abs(my) * 1.2 ? "x" : "y"
    if (axis === "x") dx = mx
  }, { passive: true })
  el.addEventListener("touchend", function() {
    if (!on) return
    on = false
    var fast = Math.abs(dx) / Math.max(1, Date.now() - t0) > 0.5
    if (axis === "x" && (Math.abs(dx) > 70 || (fast && Math.abs(dx) > 30))) { Kit.tick(); step(dx < 0 ? by : -by) }
  }, { passive: true })
  el.addEventListener("touchcancel", function() { on = false }, { passive: true })
}

// The theme: the desktop's Omarchy colours, in the phone's light or dark. When the Omarchy
// theme is the other way round from the phone, its accents go on a ground for the phone's mode
var GROUND = { dark: { background: "#17181d", foreground: "#d7d9e0", muted: "#7c8090" },
  light: { background: "#fafafa", foreground: "#212121", muted: "#8c8c8c" } }
function followThePhone() {
  var mq = window.matchMedia("(prefers-color-scheme: dark)")
  /** @type {KitTheme | null} */
  var theme = null
  function apply() {
    if (!theme) return
    var themeDark = (Kit.themeVars(theme) || { dark: false }).dark
    if (themeDark === mq.matches) { Kit.applyTheme(theme); return }
    var g = mq.matches ? GROUND.dark : GROUND.light
    Kit.applyTheme(Object.assign({}, theme, g, { dark_foreground: undefined, light_foreground: undefined, mode: mq.matches ? "dark" : "light" }))
  }
  fetch("/api/theme", { cache: "no-store" }).then(function(r) { return r.ok ? r.json() : null }).then(function(t) { theme = t; apply() }, function() {})
  if (mq.addEventListener) mq.addEventListener("change", apply)
}

// The Home Screen icon: iOS takes the one the page links when you add it, and never swaps it
// later, so offer the one for the phone's mode at that moment (or ?icon=light / ?icon=dark)
function homeIcon() {
  var asked = new URLSearchParams(location.search).get("icon")
  var light = asked ? asked === "light" : !window.matchMedia("(prefers-color-scheme: dark)").matches
  var link = /** @type {HTMLLinkElement | null} */ (document.querySelector('link[rel="apple-touch-icon"]'))
  if (link) link.href = light ? "/apple-touch-icon-light.png" : "/apple-touch-icon.png"
  var fav = /** @type {HTMLLinkElement | null} */ (document.querySelector('link[rel="icon"]'))
  if (fav) fav.href = light ? "/icon-light.svg" : "/icon.svg"
}

function start() {
  homeIcon()
  clock()
  var hash = location.hash.slice(1)
  S.selected = /^\d{4}-\d{2}-\d{2}$/.test(hash) ? hash : S.today
  // a newer page first (then it reloads), as a pull to refresh does
  $("refresh").addEventListener("click", function() {
    Kit.updateNow().then(function(reloading) { if (!reloading) refetch(L.firstOfMonth(S.selected), false) })
  })
  $("today").addEventListener("click", function() { go(S.today) })
  $("prevwk").addEventListener("click", function() { step(-7) })
  $("nextwk").addEventListener("click", function() { step(7) })
  flick($("strip"), 7)
  Kit.pullToRefresh(function() { return refetch(L.firstOfMonth(S.selected), false) })
  holdDays()
  document.addEventListener("keydown", function(ev) {
    if (document.querySelector("dialog[open]") || ev.ctrlKey || ev.metaKey || ev.altKey) return
    if (ev.key === "ArrowLeft" || ev.key === "h") step(-1)
    else if (ev.key === "ArrowRight" || ev.key === "l") step(1)
    else if (ev.key === "t") go(S.today)
  })
  // the minute turns: the clock, and the cache read again (the sync job refreshes it)
  ;(function tick() {
    setTimeout(function() {
      var wasToday = S.today
      clock()
      if (S.selected === wasToday && S.today !== wasToday) S.selected = S.today
      if (document.visibilityState === "visible") load()
      tick()
    }, 60000 - (Date.now() % 60000) + 500)
  })()
  document.addEventListener("visibilitychange", function() {
    if (document.visibilityState !== "visible") return
    var wasToday = S.today
    clock()
    if (S.today !== wasToday && S.selected === wasToday) S.selected = S.today
    load()
  })
  followThePhone()
  Kit.offline()
  Kit.autoUpdate() // a changed page reloads itself (the page's X-Kit-Page against its meta)
  render()
  load().then(ensureMonth)
}

start()
})()
