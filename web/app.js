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

var DAYS = 57 // six weeks from the month grid's first day, and a bit past (the week strip)

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
  seq: 0
}
var statusTimer = 0

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

function rangeFrom() { return L.gridStart(S.selected, sundayFirst()) }

function load() {
  var seq = ++S.seq, from = rangeFrom()
  S.loading = true
  return api("/api/agenda?from=" + from + "&days=" + DAYS).then(function(a) {
    if (seq !== S.seq) return
    var wasSunday = sundayFirst()
    S.agenda = a
    for (var i = 0; i < a.days.length; i++) S.byDate[a.days[i].date] = a.days[i]
    S.error = null
    S.loading = false
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
  $("refresh").classList.add("spin")
  if (!quiet) say("Refetching " + L.monthTitle(month) + "…")
  return post("/api/refresh?month=" + month.slice(0, 7)).then(function() {
    if (!quiet) say("Up to date")
  }, function(e) {
    say(String(e.message || e), true)
  }).then(function() {
    S.refreshing = false
    $("refresh").classList.remove("spin")
    return load()
  })
}

function say(text, bad) {
  S.status = { text: text, bad: !!bad }
  clearTimeout(statusTimer)
  statusTimer = setTimeout(function() { S.status = null; header() }, bad ? 6000 : 2500)
  header()
}

// ------------------------------------------------------------------ navigation

function select(date) {
  var month = L.firstOfMonth(S.selected)
  S.selected = date
  try { history.replaceState(null, "", "#" + (date === S.today ? "" : date)) } catch (_) {}
  render()
  if (L.firstOfMonth(date) !== month) { load(); ensureMonth() }
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

function header() {
  var t = $("tally")
  t.textContent = ""
  t.title = ""
  if (S.status) {
    t.appendChild(h("span", { "class": S.status.bad ? "bad" : "" }, S.status.text))
    return
  }
  var f = S.agenda && L.footerNext(S.agenda.next, S.today, S.now)
  if (f) {
    t.appendChild(h("span", { "class": f.soon ? "soon" : "" }, f.label))
    t.appendChild(h("b", null, f.title))
    t.appendChild(document.createTextNode(f.rest))
    t.title = f.label + f.title + f.rest
  }
}

function navButtons(onToday) {
  return h("span", { "class": "r" }, [
    h("button", { "class": "nav", type: "button", "aria-label": "Back", onclick: function() { step(-1) } }, "‹"),
    onToday ? null : h("button", { "class": "nav", type: "button", onclick: function() { select(S.today) } }, "Today"),
    h("button", { "class": "nav", type: "button", "aria-label": "Forward", onclick: function() { step(1) } }, "›")
  ])
}

function heavyBar(date) {
  var busy = L.busyMinutes(L.dayOf(S.byDate, date))
  var pct = Math.min(100, Math.round(busy / L.HEAVY_DAY_MINUTES * 100))
  return h("i", null, busy ? h("span", { "class": busy >= L.HEAVY_DAY_MINUTES ? "heavy" : "", style: "width:" + Math.max(pct, 8) + "%" }) : null)
}

// ---- Day

function day(main) {
  var sel = S.selected, first = L.weekStart(sel, sundayFirst()), letters = L.weekdayLetters(sundayFirst())
  var week = h("div", { "class": "week" })
  for (var d = 0; d < 7; d++) {
    var date = L.addDays(first, d)
    week.appendChild(h("button", {
      "class": "wd" + (date === S.today ? " today" : "") + (date === sel ? " sel" : "") + (L.isWeekend(date) ? " weekend" : ""),
      type: "button", "aria-label": L.dayTitle(date), onclick: select.bind(null, date)
    }, [h("small", null, letters[d]), h("b", null, String(Number(date.slice(8)))), heavyBar(date)]))
  }
  main.appendChild(week)

  var isToday = sel === S.today
  main.appendChild(h("div", { "class": "sect big" }, [
    h("span", null, isToday ? "Today · " + L.dayTitle(sel) : L.dayTitle(sel) + " · " + relative(sel)),
    navButtons(isToday)
  ]))

  var next = S.agenda.next
  if (isToday && next) main.appendChild(nextCard(next))

  var m = merge(L.dayOf(S.byDate, sel))
  // gaps and overlaps from the day with each duplicate once, so a copy never "overlaps" itself
  var g = L.gaps(m.day, sel, S.today, S.now, workingHours())
  var gaps = g.google.concat(g.icloud).sort(function(a, b) { return a.from - b.from })
  var clash = L.overlapping(m.day)
  var events = m.rows.map(function(r) { return r.e })
  var cn = L.currentNext(events, isToday, S.now)
  var list = h("section", { "class": "list" })
  if (!events.length) {
    var nu = nextUp(sel)
    list.appendChild(h("div", { "class": "none" }, ["Nothing scheduled", nu ? h("small", null, "next " + nu) : null]))
  }
  L.panelRows(events, gaps).forEach(function(row) {
    if (row.kind === "gap") {
      var gp = row.gap
      list.appendChild(h("div", { "class": "gap" + (gp.now ? " now" : "") },
        (gp.now ? "free now · " : "free " + L.hm(gp.from, twelve()) + " · ") + L.formatDuration(gp.minutes)))
      return
    }
    var r = m.rows[row.index], e = r.e, i = row.index
    var past = !e.allDay && (sel < S.today || (isToday && endOf(e) <= S.now))
    list.appendChild(eventRow(r, sel, {
      past: past && i !== cn.current, current: i === cn.current, next: i === cn.next,
      clash: !!clash[e.id + "@" + e.start]
    }))
  })
  main.appendChild(list)
}

// ---- One list from both calendars

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

// Both calendars as one list: rows {e, source, also: [{e, source}]}, all day first, then by
// start (Work first on a tie); a Personal event that duplicates a Work one goes in its `also`
// (the Work copy leads: it has the invite, the people and the link). `day` is the day with
// each duplicate once, for gaps and overlaps
function merge(dayData) {
  var rows = (dayData.google || []).map(function(e) { return { e: e, source: "google", also: [] } })
  var kept = []
  ;(dayData.icloud || []).forEach(function(e) {
    var twin = rows.filter(function(r) { return r.source === "google" && !r.also.length && sameEvent(r.e, e) })[0]
    if (twin) twin.also.push({ e: e, source: "icloud" })
    else { rows.push({ e: e, source: "icloud", also: [] }); kept.push(e) }
  })
  var key = function(r) { return r.e.allDay ? -1 : r.e.start }
  rows.forEach(function(r, n) { r.n = n })
  rows.sort(function(a, b) { return key(a) - key(b) || a.n - b.n })
  return { rows: rows, day: { date: dayData.date, google: dayData.google || [], icloud: kept } }
}

// "tmrw 09:00 Title": the first accepted timed event after `date` in either calendar, 14 days ahead
function nextUp(date) {
  for (var d = 1; d <= 14; d++) {
    var day = L.addDays(date, d), m = merge(L.dayOf(S.byDate, day))
    for (var i = 0; i < m.rows.length; i++) {
      var e = m.rows[i].e
      if (e.accepted && !e.allDay) return L.smartWhen(day, e.start, S.today, twelve()) + " " + e.title
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

function sourceOf(e) { return e.id.indexOf("icloud:") === 0 ? "icloud" : "google" }

function eventRow(r, date, o) {
  var e = r.e, both = r.also.length > 0
  var cls = "ev " + r.source + (both ? " both" : "") + (e.allDay ? " allday" : "") + (o.past ? " past" : "") + (o.current ? " current" : "") + (o.next ? " next" : "") +
    (!e.accepted ? " unaccepted" : "") + (e.free ? " free" : "")
  var url = e.meetingUrl || r.also.map(function(a) { return a.e.meetingUrl }).filter(Boolean)[0] || null
  var kind = L.meetingKind(url)
  var len = L.formatLength(e)
  var srcs = [r.source].concat(r.also.map(function(a) { return a.source }))
  return h("button", { "class": cls, type: "button", onclick: function() { details(e, date, r.source, r.also) } }, [
    h("div", { "class": "tm" }, [
      h("span", { "class": "srcs", title: srcs.map(label).join(" · ") }, srcs.map(function(k) { return h("i", { "class": k }) })),
      h("span", { "class": "at" }, e.allDay ? (e.spansDays ? "All day · several days" : "All day") : L.hm(e.start, twelve()) + (len ? " · " + len : "")),
      kind ? h("span", { "class": "kind" }, kind) : null,
      o.clash ? h("span", { "class": "clash" }, "overlaps") : null
    ]),
    h("div", { "class": "ti" }, e.title || "(no title)")
  ])
}

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

function swipe(el) {
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
      Kit.transition(function() { step(dx < 0 ? 1 : -1) })
    }
  }, { passive: true })
}

function start() {
  clock()
  var hash = location.hash.slice(1)
  S.selected = /^\d{4}-\d{2}-\d{2}$/.test(hash) ? hash : S.today
  $("refresh").addEventListener("click", function() { refetch(L.firstOfMonth(S.selected), false) })
  Kit.pullToRefresh(function() { return refetch(L.firstOfMonth(S.selected), false) })
  swipe($("main"))
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
  Kit.loadTheme()
  Kit.offline()
  render()
  load().then(ensureMonth)
}

start()
})()
