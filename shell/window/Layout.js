.pragma library

// The TUI's display rules (src/ui.rs, src/app.rs), ported for the window.
// Everything here is pure: it takes the JSON `calendarchy --agenda` prints
// (days keyed by date, each with google / icloud lists) and returns what to
// draw. Dates are "YYYY-MM-DD" strings, times are minutes since midnight.

var SOURCES = ["google", "icloud"]
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

// ---------------------------------------------------------------- times

function hm(minutes, twelveHour) {
  var h = Math.floor(minutes / 60), m = minutes % 60
  if (!twelveHour) return pad(h) + ":" + pad(m)
  var suffix = h >= 12 && h < 24 ? "pm" : "am"
  var h12 = h % 12 === 0 ? 12 : h % 12
  return h12 + ":" + pad(m) + suffix
}

function timeText(e, twelveHour) { return e.allDay ? "All day" : hm(e.start, twelveHour) }

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

// busy_range: timed, accepted, not free; no end = one hour
function busyRange(e) {
  if (!e.busy) return null
  var end = e.end === null || e.end === undefined ? e.start + 60 : e.end
  return [e.start, Math.min(end, DAY_MINUTES)]
}

function dayOf(byDate, date) { return byDate[date] || { date: date, google: [], icloud: [] } }

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

// Footer hints, as the TUI's second footer line
function hints(mode, e, labels, signedIn) {
  if (mode === "day") {
    var h = ["? help", "q quit", "Enter events"]
    if (!signedIn.google) h.push("g connect " + labels.google.toLowerCase())
    if (!signedIn.icloud) h.push("i connect " + labels.icloud.toLowerCase())
    return h.join(" · ")
  }
  var parts = ["? help", "q quit"]
  if (e && e.event.meetingUrl) parts.push("J join")
  if (e && e.source === "google") parts.push(e.event.accepted ? "d decline" : "a accept")
  parts.push("x delete")
  parts.push("Tab other panel", "Esc back")
  return parts.join(" · ")
}

// Bulgarian phonetic keys are their Latin twins, as keymap.rs maps them
var LATIN = {
  "й": "j", "к": "k", "л": "l", "х": "h", "Й": "J", "К": "K", "Л": "L", "Х": "H",
  "т": "t", "р": "r", "н": "n", "ф": "f", "г": "g", "и": "i", "я": "q", "а": "a",
  "д": "d", "ь": "x", "Д": "D", "С": "S", "о": "o"
}
function latin(t) { return LATIN[t] || t }
