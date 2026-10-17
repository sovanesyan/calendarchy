.pragma library

// The TUI's display rules (src/ui.rs, src/app.rs), ported for the window and
// the service's alerts (install.sh copies this file into both plugins).
// Everything here is pure: it takes the JSON `calendarchy --agenda` prints
// (days keyed by date, each with google / icloud lists) and returns what to
// draw. Dates are "YYYY-MM-DD" strings, times are minutes since midnight.

var SOURCES = ["google", "icloud"]
function otherSource(s) { return s === "google" ? "icloud" : "google" }
var DAY_MINUTES = 1440

function pad(n) { return (n < 10 ? "0" : "") + n }

function toDate(s) { return new Date(s + "T12:00:00") }
function fromDate(d) { return d.getFullYear() + "-" + pad(d.getMonth() + 1) + "-" + pad(d.getDate()) }
function addDays(s, n) { var d = toDate(s); d.setDate(d.getDate() + n); return fromDate(d) }
function dayDiff(a, b) { return Math.round((toDate(b) - toDate(a)) / 86400000) }
function firstOfMonth(s) { return s.slice(0, 8) + "01" }
function addMonths(s, n) {
  var d = toDate(firstOfMonth(s))
  d.setMonth(d.getMonth() + n)
  return fromDate(d)
}
function sameMonth(a, b) { return a.slice(0, 7) === b.slice(0, 7) }

var DAY_NAMES = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
var MONTH_NAMES = ["January", "February", "March", "April", "May", "June", "July",
  "August", "September", "October", "November", "December"]

function weekday(s) { return toDate(s).getDay() }
function isWeekend(s) { var w = weekday(s); return w === 0 || w === 6 }

// "%a %b %d": the day panel's title
function dayTitle(s) {
  var d = toDate(s)
  return DAY_NAMES[d.getDay()] + " " + MONTH_NAMES[d.getMonth()].slice(0, 3) + " " + pad(d.getDate())
}
function monthTitle(s) { var d = toDate(s); return MONTH_NAMES[d.getMonth()] + " " + d.getFullYear() }
function shortDay(s) { var d = toDate(s); return DAY_NAMES[d.getDay()] + " " + pad(d.getDate()) }

// Monday (or Sunday) on or before `s`
function weekStart(s, sundayFirst) {
  var back = (weekday(s) + (sundayFirst ? 0 : 6)) % 7
  return addDays(s, -back)
}

// The 6×7 month grid starts on the week containing the 1st
function gridStart(month, sundayFirst) { return weekStart(firstOfMonth(month), sundayFirst) }

function weekdayLetters(sundayFirst) {
  return sundayFirst ? ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"] : ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"]
}

// ISO week number of the week starting `monday`
function isoWeek(monday) {
  var d = toDate(monday)
  d.setDate(d.getDate() + 3) // Thursday of this week decides the year
  var jan4 = new Date(d.getFullYear(), 0, 4, 12)
  return 1 + Math.round(((d - jan4) / 86400000 - 3 + ((jan4.getDay() + 6) % 7)) / 7)
}

// The week-number column: the ISO week of the grid row starting `weekFirst`,
// blank for a row with no day in `month`
function weekLabel(weekFirst, month, sundayFirst) {
  if (!sameMonth(weekFirst, month) && !sameMonth(addDays(weekFirst, 6), month)) return ""
  return String(isoWeek(sundayFirst ? addDays(weekFirst, 1) : weekFirst))
}

// ---------------------------------------------------------------- times

function hm(minutes, twelveHour) {
  var h = Math.floor(minutes / 60), m = minutes % 60
  if (!twelveHour) return pad(h) + ":" + pad(m)
  var suffix = h >= 12 && h < 24 ? "pm" : "am"
  var h12 = h % 12 === 0 ? 12 : h % 12
  return h12 + ":" + pad(m) + suffix
}

function timeText(e, twelveHour) { return e.allDay ? "All day" : hm(e.start, twelveHour) }

// "09:00 – 10:00", as the details panel: the start, and the end if it has one
function timeRange(e, twelveHour) {
  if (e.allDay || e.end === null || e.end === undefined) return timeText(e, twelveHour)
  return hm(e.start, twelveHour) + " – " + hm(e.end, twelveHour)
}

// "12 min", "1 h 05", for an alert's countdown in minutes
function countdown(minutes) {
  if (minutes < 60) return minutes + " min"
  var h = Math.floor(minutes / 60), m = minutes % 60
  return m ? h + " h " + pad(m) : h + " h"
}

// format_duration: "43m", "2h 15m", "2h", "3d 2h"
function formatDuration(m) {
  if (m < 60) return m + "m"
  if (m < DAY_MINUTES) { var h = Math.floor(m / 60), r = m % 60; return r ? h + "h " + r + "m" : h + "h" }
  var d = Math.floor(m / DAY_MINUTES), hh = Math.floor((m % DAY_MINUTES) / 60)
  return hh ? d + "d " + hh + "h" : d + "d"
}

// format_length: "30m", "1h", "1h30", for timed events with an end
function formatLength(e) {
  if (e.allDay || e.spansDays || e.end === null || e.end === undefined) return ""
  var m = e.end - e.start
  if (m <= 0) return ""
  if (m < 60) return m + "m"
  var h = Math.floor(m / 60), r = m % 60
  return r ? h + "h" + pad(r) : h + "h"
}

// "zoom", "meet", "teams" or "link", from a meeting URL
function meetingKind(url) {
  if (!url) return ""
  if (url.indexOf("zoom.us") >= 0 || url.indexOf("zoommtg:") === 0) return "zoom"
  if (url.indexOf("meet.google.") >= 0) return "meet"
  if (url.indexOf("teams.") >= 0) return "teams"
  return "link"
}

// "today 09:00", "tmrw 09:00", "Wed 09:00", or "Oct 23"
function smartWhen(date, start, today, twelveHour) {
  var diff = dayDiff(today, date)
  var t = start === null || start === undefined ? "" : " " + hm(start, twelveHour)
  if (diff === 0) return "today" + t
  if (diff === 1) return "tmrw" + t
  if (diff >= 2 && diff <= 6) return DAY_NAMES[weekday(date)] + t
  var d = toDate(date)
  return MONTH_NAMES[d.getMonth()].slice(0, 3) + " " + pad(d.getDate())
}

// ---------------------------------------------------------------- busy time

// The minutes an event blocks, as the agenda decides them (busy_range:
// timed, accepted, not free; no end = one hour): [start, end] or null.
// Binaries before the range was added sent a bool, so derive it from that
function busyRange(e) {
  if (Array.isArray(e.busy)) return e.busy
  if (!e.busy) return null
  var end = e.end === null || e.end === undefined ? e.start + 60 : e.end
  return [e.start, Math.min(end, DAY_MINUTES)]
}

function dayOf(byDate, date) { return byDate[date] || { date: date, google: [], icloud: [] } }
function dayEvents(byDate, date, source) { return dayOf(byDate, date)[source] || [] }

function busyRanges(day) {
  var out = []
  for (var s = 0; s < SOURCES.length; s++) {
    var list = day[SOURCES[s]] || []
    for (var i = 0; i < list.length; i++) {
      var r = busyRange(list[i])
      if (r) out.push({ start: r[0], end: r[1], source: SOURCES[s], event: list[i] })
    }
  }
  out.sort(function(a, b) { return a.start - b.start || a.end - b.end })
  return out
}

// Busy minutes of a day, both sources merged so overlaps count once
function busyMinutes(day) {
  var ranges = busyRanges(day), total = 0, curS = -1, curE = -1
  for (var i = 0; i < ranges.length; i++) {
    var r = ranges[i]
    if (r.start > curE) { if (curE > curS) total += curE - curS; curS = r.start; curE = r.end }
    else curE = Math.max(curE, r.end)
  }
  if (curE > curS) total += curE - curS
  return total
}

var HEAVY_DAY_MINUTES = 300

// Ids of events whose busy range strictly overlaps another's (either source)
function overlapping(day) {
  var ranges = busyRanges(day), hit = {}
  for (var i = 0; i < ranges.length; i++)
    for (var j = i + 1; j < ranges.length && ranges[j].start < ranges[i].end; j++)
      if (ranges[i].start < ranges[j].end && ranges[j].start < ranges[i].end) {
        hit[ranges[i].event.id + "@" + ranges[i].start] = true
        hit[ranges[j].event.id + "@" + ranges[j].start] = true
      }
  return hit
}

// ---------------------------------------------------------------- heatmap

var HEAT_FIRST = 8, HEAT_LAST = 19

// One cell per hour 8–19 for each day of the week: two half-hour slots, each
// {count, past, off}; plus per-day early/late markers
function heatmap(byDate, weekFirst, today, now, workingHours) {
  var lo = workingHours ? workingHours[0] : 0, hi = workingHours ? workingHours[1] : DAY_MINUTES
  var days = []
  for (var d = 0; d < 7; d++) {
    var date = addDays(weekFirst, d)
    var ranges = busyRanges(dayOf(byDate, date))
    var hours = []
    for (var h = HEAT_FIRST; h <= HEAT_LAST; h++) {
      var halves = []
      for (var k = 0; k < 2; k++) {
        var s = h * 60 + k * 30, e = s + 30, count = 0
        for (var i = 0; i < ranges.length; i++) if (ranges[i].start < e && s < ranges[i].end) count++
        var past = date < today || (date === today && now >= e)
        halves.push({ count: count, past: past, off: s < lo || e > hi })
      }
      hours.push(halves)
    }
    var early = false, late = false
    for (var j = 0; j < ranges.length; j++) {
      if (ranges[j].start < HEAT_FIRST * 60) early = true
      if (ranges[j].end > (HEAT_LAST + 1) * 60) late = true
    }
    days.push({ date: date, hours: hours, marker: early && late ? "↕" : early ? "▴" : late ? "▾" : "" })
  }
  return days
}

// A day's heatmap slots as runs, for drawing continuous blocks: `busy` runs
// of consecutive busy slots with the same level (1, or 2+ overlapping) and
// the same past/future, each marked `top` / `bottom` when it starts / ends a
// busy stretch (only those ends get rounded); `work` runs of slots inside
// working hours. from/to are slot indexes (half hours from 8:00), to exclusive.
function heatRuns(hours) {
  var slots = []
  for (var h = 0; h < hours.length; h++) slots.push(hours[h][0], hours[h][1])
  var busy = [], work = []
  for (var i = 0; i < slots.length; i++) {
    var s = slots[i], last = busy[busy.length - 1]
    if (s.count > 0) {
      var level = s.count >= 2 ? 2 : 1
      if (last && last.to === i && last.level === level && last.past === s.past) last.to = i + 1
      else busy.push({ from: i, to: i + 1, level: level, past: s.past })
    }
    var w = work[work.length - 1]
    if (!s.off) {
      if (w && w.to === i) w.to = i + 1
      else work.push({ from: i, to: i + 1 })
    }
  }
  for (var k = 0; k < busy.length; k++) {
    busy[k].top = k === 0 || busy[k - 1].to !== busy[k].from
    busy[k].bottom = k === busy.length - 1 || busy[k + 1].from !== busy[k].to
  }
  return { busy: busy, work: work }
}

// ---------------------------------------------------------------- event panels

// Current and next per panel, on today only: current is the last event under
// way before the first one still to come; next is the first with start > now
function currentNext(events, isToday, now) {
  var res = { current: -1, next: -1 }
  if (!isToday) return res
  for (var i = 0; i < events.length; i++) {
    var e = events[i]
    if (e.allDay) continue
    if (e.start > now) { if (res.next < 0) res.next = i; break }
    var end = e.end === null || e.end === undefined ? e.start : e.end
    if (now < end || (e.end === DAY_MINUTES)) res.current = i
  }
  return res
}

// Free gaps between busy blocks (both sources), as the TUI lists them: only
// between two blocks, at least 30 min (5 when under way), working hours
// clamped, none on past days; each belongs to the panel whose event closed
// the block before it
function gaps(day, date, today, now, workingHours) {
  var out = { google: [], icloud: [] }
  if (date < today) return out
  var from = date === today ? now : 0
  var lo = workingHours ? workingHours[0] : 0, hi = workingHours ? workingHours[1] : DAY_MINUTES
  var ranges = busyRanges(day)
  var blocks = []
  for (var i = 0; i < ranges.length; i++) {
    var r = ranges[i], last = blocks[blocks.length - 1]
    if (last && r.start < last.end) {
      if (r.end > last.end) { last.end = r.end; last.source = r.source }
    } else if (last && r.start === last.end) {
      last.end = Math.max(last.end, r.end); last.source = r.source
    } else blocks.push({ start: r.start, end: r.end, source: r.source })
  }
  for (var b = 0; b + 1 < blocks.length; b++) {
    var blockEnd = blocks[b].end, nextStart = blocks[b + 1].start
    var freeFrom = Math.max(blockEnd, from, lo), freeTo = Math.min(nextStart, hi)
    var underWay = from > blockEnd && from > lo
    var len = freeTo - freeFrom
    if (len >= (underWay ? 5 : 30)) out[blocks[b].source].push({ from: freeFrom, minutes: len, now: underWay })
  }
  return out
}

// A panel's rows: events, with each gap after the last event that starts
// before it
function panelRows(events, panelGaps) {
  var rows = [], g = 0
  for (var i = 0; i < events.length; i++) {
    rows.push({ kind: "event", index: i, event: events[i] })
    var nextStart = i + 1 < events.length && !events[i + 1].allDay ? events[i + 1].start : 99999
    while (g < panelGaps.length && !events[i].allDay && events[i].start < panelGaps[g].from && nextStart >= panelGaps[g].from) {
      rows.push({ kind: "gap", gap: panelGaps[g] })
      g++
    }
  }
  while (g < panelGaps.length) rows.push({ kind: "gap", gap: panelGaps[g++] })
  return rows
}

// "Nothing scheduled · next Mon 09:00 Title": same source, 14 days ahead
function nextUp(byDate, source, selected, today, twelveHour) {
  for (var d = 1; d <= 14; d++) {
    var date = addDays(selected, d)
    var list = dayOf(byDate, date)[source] || []
    for (var i = 0; i < list.length; i++)
      if (list[i].accepted && !list[i].allDay) return smartWhen(date, list[i].start, today, twelveHour) + " " + list[i].title
  }
  return ""
}

// Coming up: the days after the selected one, accepted timed one-day events
// from both sources by start
function comingUp(byDate, selected, count) {
  var out = []
  for (var d = 1; d <= count; d++) {
    var date = addDays(selected, d), day = dayOf(byDate, date), items = []
    for (var s = 0; s < SOURCES.length; s++) {
      var list = day[SOURCES[s]] || []
      for (var i = 0; i < list.length; i++) {
        var e = list[i]
        if (e.accepted && !e.allDay && !e.spansDays) items.push(e)
      }
    }
    items.sort(function(a, b) { return a.start - b.start })
    out.push({ date: date, items: items })
  }
  return out
}

// The TUI's fitting rule for a Coming up line, in characters: items joined
// by " · ", 5 reserved for "+N" while more follow, stop when under 12 left
function fitLine(items, budget, twelveHour) {
  var parts = [], room = budget
  for (var i = 0; i < items.length; i++) {
    var more = i + 1 < items.length
    var avail = room - (more ? 5 : 0) - (parts.length ? 3 : 0)
    if (avail < 12) return { parts: parts, more: items.length - i }
    var t = hm(items[i].start, twelveHour), text = t + " " + items[i].title
    if (text.length > avail) text = text.slice(0, avail - 1) + "…"
    parts.push({ time: t, title: text.slice(t.length + 1) })
    room -= text.length + (parts.length > 1 ? 3 : 0)
  }
  return { parts: parts, more: 0 }
}

// ---------------------------------------------------------------- navigation (app.rs)

// A selection is { date, source, index }; null means there's nothing to
// select (back to day mode) or nowhere to move

function hasEvents(byDate, date) {
  var d = byDate[date]
  return !!d && ((d.google || []).length + (d.icloud || []).length) > 0
}

// After a reload: keep the selection on an event that's still there
function clampSelection(byDate, date, source, index) {
  var list = dayEvents(byDate, date, source)
  if (list.length === 0) {
    var other = otherSource(source)
    return dayEvents(byDate, date, other).length ? { date: date, source: other, index: 0 } : null
  }
  return { date: date, source: source, index: Math.min(index, list.length - 1) }
}

// Enter: on today the event under way, else the next one (earliest of the
// two panels); otherwise the first event, Work before Personal
function enterEvents(byDate, date, today, now) {
  var g = dayEvents(byDate, date, "google"), i = dayEvents(byDate, date, "icloud")
  if (g.length === 0 && i.length === 0) return null
  var at = function(source, index) { return { date: date, source: source, index: index } }
  if (date === today) {
    var gc = currentNext(g, true, now), ic = currentNext(i, true, now)
    if (gc.current >= 0) return at("google", gc.current)
    if (ic.current >= 0) return at("icloud", ic.current)
    if (gc.next >= 0 && (ic.next < 0 || g[gc.next].start <= i[ic.next].start)) return at("google", gc.next)
    if (ic.next >= 0) return at("icloud", ic.next)
  }
  return at(g.length ? "google" : "icloud", 0)
}

// j: down the panel, on to Personal, then to the next day with events (as
// far as what's loaded goes)
function nextEvent(byDate, date, source, index) {
  var g = dayEvents(byDate, date, "google"), i = dayEvents(byDate, date, "icloud")
  if (source === "google" && index + 1 < g.length) return { date: date, source: source, index: index + 1 }
  if (source === "google" && i.length) return { date: date, source: "icloud", index: 0 }
  if (source === "icloud" && index + 1 < i.length) return { date: date, source: source, index: index + 1 }
  for (var d = 1; d <= 90; d++) {
    var next = addDays(date, d)
    if (!byDate[next]) break
    if (hasEvents(byDate, next))
      return { date: next, source: dayEvents(byDate, next, "google").length ? "google" : "icloud", index: 0 }
  }
  return null
}

// k: up the panel, back to Work, then to the last event of the day before
function prevEvent(byDate, date, source, index) {
  var g = dayEvents(byDate, date, "google")
  if (index > 0) return { date: date, source: source, index: index - 1 }
  if (source === "icloud" && g.length) return { date: date, source: "google", index: g.length - 1 }
  for (var d = 1; d <= 90; d++) {
    var prev = addDays(date, -d)
    if (!byDate[prev]) break
    if (hasEvents(byDate, prev)) {
      var pi = dayEvents(byDate, prev, "icloud").length
      return pi ? { date: prev, source: "icloud", index: pi - 1 }
        : { date: prev, source: "google", index: dayEvents(byDate, prev, "google").length - 1 }
    }
  }
  return null
}

// Tab: the other panel's first event at or after this one, else its last
function switchPanel(byDate, date, source, index) {
  var other = otherSource(source)
  var list = dayEvents(byDate, date, other)
  if (list.length === 0) return null
  var cur = dayEvents(byDate, date, source)[index]
  var key = function(e) { return e.allDay ? -1 : e.start }
  var target = list.length - 1
  if (cur) for (var j = 0; j < list.length; j++) if (key(list[j]) >= key(cur)) { target = j; break }
  return { date: date, source: other, index: target }
}

// ---------------------------------------------------------------- actions

// The event actions, by the verb `calendarchy --event` takes
var VERBS = {
  accept: { name: "Accept", prompt: "Accept this event?", doing: "Accepting event...", done: "Event accepted" },
  decline: { name: "Decline", prompt: "Decline this event?", doing: "Declining event...", done: "Event declined" },
  "delete": { name: "Delete", prompt: "Delete this event?", doing: "Deleting event...", done: "Event deleted" }
}

// ---------------------------------------------------------------- alerts

// Events eligible for an alert at the agenda's `now`: today's busy timed
// events starting within `lead` minutes, each keyed by id + date + start
function dueAlerts(agenda, lead) {
  var due = []
  if (!agenda || !agenda.days || agenda.days.length === 0) return due
  var day = agenda.days[0]
  if (day.date !== agenda.today) return due
  for (var s = 0; s < SOURCES.length; s++) {
    var events = day[SOURCES[s]] || []
    for (var i = 0; i < events.length; i++) {
      var e = events[i]
      var busy = busyRange(e)
      if (!busy) continue
      var until = busy[0] - agenda.now
      if (until >= 0 && until <= lead) due.push({ key: e.id + "@" + day.date + "@" + e.start, event: e, until: until })
    }
  }
  return due
}

// ---------------------------------------------------------------- details

function decodeEntities(s) {
  return s.replace(/&nbsp;/g, " ").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, "\"")
    .replace(/&#39;|&apos;/g, "'").replace(/&#(\d+);/g, function(_, n) { return String.fromCharCode(Number(n)) })
    .replace(/&amp;/g, "&")
}

// The TUI's HTML-to-text for descriptions
function descriptionText(html) {
  if (!html) return ""
  var t = html.replace(/\r/g, "")
    .replace(/<\s*(br|\/p|\/div|\/tr|\/h[1-3])\b[^>]*>/gi, "\n")
    .replace(/<\s*li\b[^>]*>/gi, "\n• ")
    .replace(/<[^>]+>/g, "")
  t = decodeEntities(t).replace(/[ \t]+\n/g, "\n").replace(/\n{3,}/g, "\n\n")
  return t.trim()
}

function attendeeSummary(list) {
  var going = 0, maybe = 0, declined = 0, none = 0
  for (var i = 0; i < list.length; i++) {
    var s = list[i].status
    if (s === "accepted" || s === "organizer") going++
    else if (s === "tentative") maybe++
    else if (s === "declined") declined++
    else none++
  }
  var parts = []
  if (going) parts.push(going + " going")
  if (maybe) parts.push(maybe + " maybe")
  if (declined) parts.push(declined + " declined")
  if (none) parts.push(none + " no reply")
  return parts.join(" · ")
}

// ---------------------------------------------------------------- footer

// "Now: Title" / "Next: Title in 2h 15m", from the agenda's next
function footerNext(next, today, now) {
  if (!next) return null
  var title = next.title.length > 30 ? next.title.slice(0, 29) + "…" : next.title
  if (next.current) return { label: "Now: ", title: title, rest: "", soon: true }
  var minutes = next.date === today
    ? next.start - now
    : (DAY_MINUTES - now) + (dayDiff(today, next.date) - 1) * DAY_MINUTES + next.start
  if (minutes <= 0) return { label: "Now: ", title: title, rest: "", soon: true }
  return { label: "Next: ", title: title, rest: " in " + formatDuration(minutes), soon: minutes <= 15 }
}

// Footer hints, as the TUI's second footer line, with the keys as bound
// (`tables` from windowKeys); a hint whose action has no key is left out
function hints(mode, e, sources, tables) {
  var t = tables[mode] || {}
  var parts = []
  var hint = function(action, text) { var k = keyLabel(t, action); if (k) parts.push(k + " " + text) }
  hint("help", "help")
  hint("quit", "quit")
  if (mode === "day") {
    hint("enter_events", "events")
    if (!sources.google.configured) hint("connect_google", "connect " + sources.google.label.toLowerCase())
    if (!sources.icloud.configured) hint("connect_icloud", "connect " + sources.icloud.label.toLowerCase())
    return parts.join(" · ")
  }
  if (e && e.event.meetingUrl) hint("join", "join")
  if (e && e.source === "google") { if (e.event.accepted) hint("decline", "decline"); else hint("accept", "accept") }
  hint("delete", "delete")
  hint("switch_panel", "other panel")
  hint("exit_events", "back")
  return parts.join(" · ")
}

// ---------------------------------------------------------------- keys (keymap.rs)

// The agenda's `keys` are the bindings in effect, config overrides applied:
// per mode ("day", "event"), action name → key specs as config writes them
// ("J", "ctrl+d", "enter", "esc", "tab", "shift+tab", "left", "space",
// "f5"). These are the defaults (Keymap::effective with no overrides), for
// an agenda from a binary that doesn't print them.
var DEFAULT_KEYS = {
  day: {
    connect_google: ["g"], connect_icloud: ["i"], enter_events: ["enter"], help: ["?"],
    next_day: ["l", "right"], next_month: ["L", "ctrl+d"], next_week: ["j", "down"], now: ["n"],
    open_google_web: ["1"], open_icloud_web: ["2"], prev_day: ["h", "left"], prev_month: ["H", "ctrl+u"],
    prev_week: ["k", "up"], quit: ["q"], refresh: ["r"], search: ["f"], setup: ["S"], today: ["t"],
    toggle_logs: ["D"]
  },
  event: {
    accept: ["a"], decline: ["d"], "delete": ["x"], exit_events: ["esc"], help: ["?"], join: ["J"],
    jump_events_back: ["ctrl+u"], jump_events_forward: ["ctrl+d"], next_event: ["j", "down"],
    next_month: ["L"], now: ["n"], open_google_web: ["1"], open_icloud_web: ["2"],
    prev_event: ["k", "up"], prev_month: ["H"], quit: ["q"], refresh: ["r"], search: ["f"],
    setup: ["S"], switch_panel: ["tab", "shift+tab"], today: ["t"], toggle_logs: ["D"]
  }
}

// The window's own keys on top of the TUI's, each only while its key is free
// in that mode: Enter joins (the TUI has Enter for nothing on an event), Esc
// closes from the days (the TUI never quits on Esc; a window can)
var WINDOW_KEYS = { day: { quit: ["esc"] }, event: { join: ["enter"] } }

var NAMED_KEYS = {
  enter: "enter", "return": "enter", esc: "esc", escape: "esc", tab: "tab",
  backtab: "shift+tab", "shift+tab": "shift+tab", "s-tab": "shift+tab", space: "space",
  left: "left", right: "right", up: "up", down: "down", backspace: "backspace",
  home: "home", end: "end", pageup: "pageup", pagedown: "pagedown"
}

// Key::parse then Key::spec: a config key spec in its canonical form ("^d",
// "C-d", "Ctrl+d" → "ctrl+d"; "Return" → "enter"; " " → "space"), or null
// when it isn't one. Matching is on canonical specs.
function canonicalKey(spec) {
  if (typeof spec !== "string") return null
  spec = spec.trim()
  var lower = spec.toLowerCase()
  var prefixes = ["ctrl+", "ctrl-", "c-", "^"]
  for (var p = 0; p < prefixes.length; p++) {
    if (lower.indexOf(prefixes[p]) === 0 && spec.length > prefixes[p].length) {
      var rest = canonicalKey(spec.slice(prefixes[p].length))
      return rest === null ? null : rest.indexOf("ctrl+") === 0 ? rest : "ctrl+" + rest
    }
  }
  var chars = Array.from(spec)
  if (chars.length === 1) return chars[0] === " " ? "space" : chars[0]
  if (NAMED_KEYS.hasOwnProperty(lower)) return NAMED_KEYS[lower]
  var f = /^f(\d+)$/.exec(lower)
  if (f && Number(f[1]) >= 1 && Number(f[1]) <= 12) return "f" + Number(f[1])
  return null
}

// Bulgarian phonetic keys are their Latin twins, as keymap.rs's latin()
var LATIN = {
  "й": "j", "к": "k", "л": "l", "х": "h", "Й": "J", "К": "K", "Л": "L", "Х": "H",
  "т": "t", "р": "r", "н": "n", "ф": "f", "г": "g", "и": "i",
  "я": "q", "а": "a", "д": "d", "ь": "x", "Д": "D", "С": "S"
}
function latin(t) { return LATIN[t] || t }

// Qt::Key and Qt::KeyboardModifier values (stable across Qt versions),
// spelled out so this stays plain JS
var QT_KEYS = {
  0x01000000: "esc", 0x01000001: "tab", 0x01000002: "shift+tab", 0x01000003: "backspace",
  0x01000004: "enter", 0x01000005: "enter", 0x01000010: "home", 0x01000011: "end",
  0x01000012: "left", 0x01000013: "up", 0x01000014: "right", 0x01000015: "down",
  0x01000016: "pageup", 0x01000017: "pagedown", 0x20: "space"
}
var QT_KEY_F1 = 0x01000030
var QT_SHIFT = 0x02000000, QT_CTRL = 0x04000000

// A QML key event (event.key, event.text, event.modifiers) as a canonical
// spec, as action_for sees a crossterm key: Ctrl is kept, other modifiers
// are dropped (Shift is already in the character: "J" vs "j", and makes Tab
// "shift+tab"), Bulgarian phonetic letters become Latin. Null for a key with
// nothing to match (a bare modifier, a dead key).
function pressedKey(key, text, modifiers) {
  var ctrl = (modifiers & QT_CTRL) !== 0, shift = (modifiers & QT_SHIFT) !== 0
  var base = QT_KEYS[key] || null
  if (base === "tab" && shift) base = "shift+tab"
  if (!base && key >= QT_KEY_F1 && key < QT_KEY_F1 + 12) base = "f" + (key - QT_KEY_F1 + 1)
  if (!base) {
    // The text has the case; with Ctrl it's a control character, so go by
    // the key code (an upper-case letter, for Cyrillic too) and Shift
    var c = ""
    if (!ctrl && text && text.charCodeAt(0) >= 0x20 && text !== "\u007f") c = text
    else if (key > 0x20 && key < 0x01000000) {
      c = String.fromCharCode(key)
      c = shift ? c.toUpperCase() : c.toLowerCase()
    }
    if (Array.from(c).length !== 1) return null
    base = latin(c)
  }
  return ctrl ? "ctrl+" + base : base
}

// The bindings the window uses, per mode: the agenda's `keys` (or the
// defaults, when it has none) plus WINDOW_KEYS where their key is free.
// Specs come out canonical; ones that can't be read are dropped.
function windowKeys(keys) {
  var out = {}
  var modes = ["day", "event"]
  for (var m = 0; m < modes.length; m++) {
    var mode = modes[m]
    var from = keys && keys[mode] ? keys[mode] : DEFAULT_KEYS[mode]
    var table = {}, taken = {}
    for (var action in from) {
      var specs = []
      for (var i = 0; i < from[action].length; i++) {
        var k = canonicalKey(from[action][i])
        if (k !== null && !taken[k]) { specs.push(k); taken[k] = true }
      }
      if (specs.length) table[action] = specs
    }
    var extra = WINDOW_KEYS[mode]
    for (var a in extra)
      for (var j = 0; j < extra[a].length; j++)
        if (!taken[extra[a][j]]) { taken[extra[a][j]] = true; (table[a] = table[a] || []).push(extra[a][j]) }
    out[mode] = table
  }
  return out
}

// windowKeys turned around: per mode, canonical spec → action
function keyIndex(tables) {
  var out = {}
  for (var mode in tables) {
    var index = {}
    for (var action in tables[mode])
      for (var i = 0; i < tables[mode][action].length; i++) index[tables[mode][action][i]] = action
    out[mode] = index
  }
  return out
}

// What a pressed key does in `mode`: an action name, or null
function actionFor(index, mode, key, text, modifiers) {
  var k = pressedKey(key, text, modifiers)
  var table = index[mode] || {}
  return k !== null && table.hasOwnProperty(k) ? table[k] : null
}

// Key::label: "l", "^d", "←", "Enter", "S-Tab", "Space", "F5"
var KEY_LABELS = {
  space: "Space", left: "←", right: "→", up: "↑", down: "↓", enter: "Enter", esc: "Esc",
  tab: "Tab", "shift+tab": "S-Tab", backspace: "Bksp", home: "Home", end: "End",
  pageup: "PgUp", pagedown: "PgDn"
}
function specLabel(spec) {
  if (spec.indexOf("ctrl+") === 0 && spec.length > 5) return "^" + specLabel(spec.slice(5))
  if (KEY_LABELS.hasOwnProperty(spec)) return KEY_LABELS[spec]
  if (/^f\d+$/.test(spec)) return "F" + spec.slice(1)
  return spec
}

// The first key of an action in one mode's table, as a label ("" if none)
function keyLabel(table, action) { return table[action] && table[action].length ? specLabel(table[action][0]) : "" }

// Keymap::keys_for: an action's keys across both modes, day mode's first
function keysFor(tables, action) {
  var out = []
  var modes = ["day", "event"]
  for (var m = 0; m < modes.length; m++) {
    var list = (tables[modes[m]] || {})[action] || []
    for (var i = 0; i < list.length; i++) if (out.indexOf(list[i]) < 0) out.push(list[i])
  }
  return out
}

// Keymap::label_for: a group of related actions' keys side by side, "h/l ←/→"
// for [prev_day, next_day], or each action's keys in turn when they don't
// pair up (skipping actions left without a key)
function labelFor(tables, actions) {
  var keys = actions.map(function(a) { return keysFor(tables, a) })
  var n = keys[0].length
  if (keys.every(function(k) { return k.length === n })) {
    var cols = []
    for (var i = 0; i < n; i++) cols.push(keys.map(function(k) { return specLabel(k[i]) }).join("/"))
    return cols.join(" ")
  }
  return keys.filter(function(k) { return k.length }).map(function(k) { return k.map(specLabel).join(" ") }).join(" / ")
}

// The help overlay, as the TUI's (ui.rs render_help_modal) with the window's
// differences: [actions, description], a section with no actions
var HELP = [
  ["Navigate"],
  [["prev_day", "next_day"], "previous / next day"],
  [["prev_week", "next_week"], "previous / next week"],
  [["prev_month", "next_month"], "previous / next month"],
  [["enter_events", "exit_events"], "browse events / back"],
  [["today", "now"], "go to today / current event"],
  ["Events"],
  [["prev_event", "next_event"], "previous / next event"],
  [["jump_events_back", "jump_events_forward"], "jump 10 events"],
  [["switch_panel"], "other panel"],
  [["join"], "join meeting & close"],
  [["accept", "decline", "delete"], "accept / decline / delete"],
  ["Search & misc"],
  [["search"], "search titles & people, in the TUI"],
  [["refresh", "toggle_logs", "setup"], "refresh / logs, setup in the TUI"],
  [["connect_google", "connect_icloud"], "connect Google / iCloud, in the TUI"],
  [["open_google_web", "open_icloud_web"], "open Google / iCloud in browser"],
  [["quit"], "close"]
]

// The help overlay's rows from the bindings in use: { keys, text, section };
// a row whose actions have no key at all is left out
function helpRows(tables) {
  var rows = []
  for (var i = 0; i < HELP.length; i++) {
    var h = HELP[i]
    if (h.length === 1) { rows.push({ keys: h[0], text: "", section: true }); continue }
    var label = labelFor(tables, h[0])
    if (label) rows.push({ keys: label, text: h[1], section: false })
  }
  rows.push({ keys: "", text: "Bulgarian phonetic keys work too · any key closes", section: false })
  return rows
}
