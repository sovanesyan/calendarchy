import QtQuick
import Quickshell
import Quickshell.Io
import "Layout.js" as L

// Runs once for the shell, with no UI of its own. `calendarchy --refresh`
// every few minutes keeps the cache fresh (for the window, the TUI's first
// frame, the TRMNL push), and `calendarchy --agenda` each minute (it reads
// the disk cache only) finds meetings to alert about. Alerts go through the
// shell's notifications: a click joins (Zoom straight into the app) or opens
// calendarchy.
Item {
  id: root
  width: 0
  height: 0
  visible: false

  property string command: "calendarchy"
  property int refreshMinutes: 5
  property int alertMinutes: 5

  property var agenda: null

  // Alerts already sent, by event id + date + start, so a re-run doesn't
  // alert twice for the same start. Kept in the runtime dir as well, so a
  // shell reload (every plugin edit is one) doesn't alert again either.
  property var alerted: ({})

  function load() {
    if (agendaProc.running) return
    agendaProc.command = [root.command, "--agenda", "--days", "1"]
    agendaProc.running = true
  }

  function refresh() {
    if (refreshProc.running) return
    refreshProc.command = [root.command, "--refresh"]
    refreshProc.running = true
  }

  function absorb(text) {
    var data
    try {
      data = JSON.parse(text)
    } catch (e) {
      return
    }
    if (!data || !data.days) return
    root.agenda = data
    root.sendAlerts()
  }

  function sendAlerts() {
    var due = L.dueAlerts(root.agenda, root.alertMinutes)
    if (due.length === 0) return
    var seen = {}
    var today = "@" + root.agenda.today + "@"
    for (var key in root.alerted) if (key.indexOf(today) >= 0) seen[key] = true
    var sent = false
    for (var i = 0; i < due.length; i++) {
      if (seen[due[i].key]) continue
      seen[due[i].key] = true
      sent = true
      var e = due[i].event
      var headline = e.title + (due[i].until > 0 ? " in " + L.countdown(due[i].until) : " now")
      var parts = [L.timeRange(e, root.agenda.twelveHour)]
      var kind = L.meetingKind(e.meetingUrl)
      if (kind) parts.push(kind)
      else if (e.location) parts.push(e.location)
      var exec = e.joinUrl
        ? ["xdg-open", e.joinUrl]
        : ["omarchy-launch-or-focus-tui", root.command]
      // U+F00ED calendar, as a surrogate pair
      Quickshell.execDetached(["omarchy-notification-send", "--app-name", "Calendarchy",
        "-g", "󰃭", "-u", "normal", headline, parts.join(" · "), "--exec"].concat(exec))
    }
    root.alerted = seen
    if (sent) alertedFile.setText(JSON.stringify(seen))
  }

  FileView {
    id: alertedFile
    path: Quickshell.env("XDG_RUNTIME_DIR") + "/calendarchy-alerted.json"
    // Read before the first agenda, so nothing already sent goes out again
    blockLoading: true
    printErrors: false
    onLoaded: {
      try {
        root.alerted = JSON.parse(text()) || {}
      } catch (e) {
      }
    }
  }

  // Each minute, just after the minute turns
  Timer {
    id: minuteTimer
    running: true
    triggeredOnStart: true
    onTriggered: {
      root.load()
      interval = 60000 - (Date.now() % 60000) + 500
      restart()
    }
  }

  Timer {
    interval: Math.max(1, root.refreshMinutes) * 60000
    running: true
    repeat: true
    triggeredOnStart: true
    onTriggered: root.refresh()
  }

  Process {
    id: agendaProc
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: root.absorb(text)
    }
  }

  Process {
    id: refreshProc
    onExited: root.load()
  }
}
