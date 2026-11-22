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

// The selected week and two after it: the strip, the day, and "next" two weeks ahead.
// Asked every minute with the last ETag, so an unchanged minute is a 304 with no body
var DAYS = 21

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
  var seq = ++S.seq, from = rangeFrom(), url = "/api/agenda?from=" + from + "&days=" + DAYS
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

function step(n) { select(L.addDays(S.selected, n)) }

// ------------------------------------------------------------------ drawing

function render() {
  header()
  var main = $("main")
  main.textContent = ""
  if (!S.agenda) {
    main.appendChild(S.error ? h("div", { "class": "err" }, "Couldn't load the calendar: " + S.error) : h("div", { "class": "empty" }, "Loading…"))
    return
  }
  if (S.error) main.appendChild(h("div", { "class": "msg bad" }, S.error))
  day(main)
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
  var week = $("week")
  week.textContent = ""
  for (var i = 0; i < 7; i++) {
    var date = L.addDays(first, i)
    week.appendChild(h("button", {
      "class": "wd" + (date === S.today ? " today" : "") + (date === sel ? " sel" : "") + (L.isWeekend(date) ? " weekend" : ""),
      type: "button", "aria-label": L.dayTitle(date), "aria-pressed": String(date === sel), onclick: select.bind(null, date)
    }, [h("small", null, letters[i]), h("b", null, String(Number(date.slice(8)))), heavyBar(date)]))
  }
}

function heavyBar(date) {
  var busy = L.busyMinutes(L.dayOf(S.byDate, date))
  var pct = Math.min(100, Math.round(busy / L.HEAVY_DAY_MINUTES * 100))
  return h("i", null, busy ? h("span", { "class": busy >= L.HEAVY_DAY_MINUTES ? "heavy" : "", style: "width:" + Math.max(pct, 8) + "%" }) : null)
}

// ---- Day

function day(main) {
  var sel = S.selected, isToday = sel === S.today
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
    S.scrolledFor = sel
    main.appendChild(h("div", { "class": "none" }, [allDay.length ? "Nothing at a set time" : "Nothing scheduled", nu ? h("small", null, "Next: " + nu) : null]))
    return
  }
  main.appendChild(timeline(m, timed, sel, isToday))
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

function timeline(m, rows, sel, isToday) {
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
  if (S.scrolledFor !== sel) {
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
  if (!twelve()) return String(hr)
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
  dlg.appendChild(h("div", { "class": "kit-sheet-box" }, body))
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

// A sideways swipe on `el` moves `by` days (the timeline a day, the week strip a week)
function swipe(el, by) {
  var x0 = 0, y0 = 0, t0 = 0, on = false
  el.addEventListener("touchstart", function(ev) {
    if (ev.touches.length !== 1 || document.querySelector("dialog[open]")) { on = false; return }
    on = true; x0 = ev.touches[0].clientX; y0 = ev.touches[0].clientY; t0 = Date.now()
  }, { passive: true })
  el.addEventListener("touchend", function(ev) {
    if (!on) return
    on = false
    var t = ev.changedTouches[0], dx = t.clientX - x0, dy = t.clientY - y0
    if (Math.abs(dx) > 70 && Math.abs(dy) < Math.abs(dx) * 0.6 && Date.now() - t0 < 800) {
      Kit.tick()
      Kit.transition(function() { step(dx < 0 ? by : -by) })
    }
  }, { passive: true })
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
  $("refresh").addEventListener("click", function() { refetch(L.firstOfMonth(S.selected), false) })
  $("today").addEventListener("click", function() { select(S.today) })
  $("prevwk").addEventListener("click", function() { step(-7) })
  $("nextwk").addEventListener("click", function() { step(7) })
  swipe($("strip"), 7)
  Kit.pullToRefresh(function() { return refetch(L.firstOfMonth(S.selected), false) })
  swipe($("main"), 1)
  document.addEventListener("keydown", function(ev) {
    if (document.querySelector("dialog[open]") || ev.ctrlKey || ev.metaKey || ev.altKey) return
    if (ev.key === "ArrowLeft" || ev.key === "h") step(-1)
    else if (ev.key === "ArrowRight" || ev.key === "l") step(1)
    else if (ev.key === "t") select(S.today)
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
