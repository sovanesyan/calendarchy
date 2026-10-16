.pragma library

// Pure helpers for the calendarchy service's alerts: everything here works
// on the JSON that `calendarchy --agenda` prints.

function pad(n) { return (n < 10 ? "0" : "") + n }

// Minutes since midnight as a clock time; 1440 is "24:00" (end of day)
function hm(minutes, twelveHour) {
  var h = Math.floor(minutes / 60), m = minutes % 60
  if (!twelveHour) return pad(h) + ":" + pad(m)
  var suffix = h >= 12 && h < 24 ? "pm" : "am"
  var h12 = h % 12 === 0 ? 12 : h % 12
  return h12 + (m ? ":" + pad(m) : "") + suffix
}

function timeRange(e, twelveHour) {
  if (e.allDay) return "all day"
  var s = hm(e.start, twelveHour)
  return e.end !== null && e.end !== undefined && e.end !== e.start ? s + "–" + hm(e.end, twelveHour) : s
}

// "zoom", "meet", "teams" or "link", from a meeting URL
function meetingKind(url) {
  if (!url) return ""
  if (url.indexOf("zoom.") >= 0) return "zoom"
  if (url.indexOf("meet.google.") >= 0) return "meet"
  if (url.indexOf("teams.") >= 0) return "teams"
  return "link"
}

// "12 min", "1 h 05", for a countdown in minutes
function countdown(minutes) {
  if (minutes < 60) return minutes + " min"
  var h = Math.floor(minutes / 60), m = minutes % 60
  return m ? h + " h " + pad(m) : h + " h"
}

// Events eligible for an alert at `now`: busy, timed, starting within `lead`
function dueAlerts(agenda, lead) {
  var due = []
  if (!agenda || !agenda.days || agenda.days.length === 0) return due
  var day = agenda.days[0]
  if (day.date !== agenda.today) return due
  var sources = ["google", "icloud"]
  for (var s = 0; s < sources.length; s++) {
    var events = day[sources[s]] || []
    for (var i = 0; i < events.length; i++) {
      var e = events[i]
      if (!e.busy || e.allDay) continue
      var until = e.start - agenda.now
      if (until >= 0 && until <= lead) due.push({ key: e.id + "@" + day.date + "@" + e.start, event: e, until: until })
    }
  }
  return due
}
