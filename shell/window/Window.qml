import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui
import "Layout.js" as L

// calendarchy as a window: the TUI's layout, panel for panel, drawn with the
// shell's theme. Left, the month and the selected week's busy heatmap; in the
// middle, the selected day with Work above Personal, the free gaps between
// them, and Coming up; on the right, in event mode, the event's details. The
// footer says what's next and which keys do what. The keys are the TUI's,
// config overrides included: the agenda says what's bound to what.
//
// Data comes from calendarchy itself: `--agenda` for the visible six weeks,
// `--refresh --month` for months nobody else keeps fresh, and `--event` for
// accept / decline / delete. Summon with
//   omarchy-shell shell toggle local.calendarchy.window '{}'
Item {
  id: root

  property var shell: null

  readonly property string pluginId: "local.calendarchy.window"
  readonly property string command: "calendarchy"

  readonly property color fg: Color.foreground
  readonly property color bg: Color.background
  readonly property color dim: Qt.rgba(fg.r, fg.g, fg.b, 0.55)
  readonly property color faint: Qt.rgba(fg.r, fg.g, fg.b, 0.18)
  readonly property string font: Style.font.family
  readonly property real fs: Style.font.body
  readonly property real small: Style.font.bodySmall
  readonly property real caption: Style.font.caption

  // ------------------------------------------------------------ lifecycle

  function open(payloadJson) {
    var date = ""
    try {
      var p = JSON.parse(String(payloadJson || "{}"))
      if (p && typeof p.date === "string") date = p.date
    } catch (e) {}
    startOn(date || L.fromDate(new Date()))
    window.visible = true
    Qt.callLater(function() { keys.forceActiveFocus() })
  }

  // The host's hide() calls close(); closing here calls hide() (from
  // onVisibleChanged), whose close() then finds the window already hidden
  function close() {
    window.visible = false
  }

  function requestClose() { close() }

  // ------------------------------------------------------------ state

  property string today: L.fromDate(new Date())
  property int now: new Date().getHours() * 60 + new Date().getMinutes()
  property string month: L.firstOfMonth(today)
  property string selected: today
  property string mode: "day"              // "day" or "event", as the TUI
  property string selSource: "google"
  property int selIndex: 0
  property var pending: null               // { verb, id, prompt }
  property bool showHelp: false
  property string status: ""
  property bool statusError: false

  property var agenda: null                // the visible six weeks (and a bit), and next
  property var byDate: ({})
  property var refreshedMonths: ({})

  readonly property bool twelveHour: agenda ? agenda.twelveHour : false
  readonly property bool sundayFirst: agenda ? agenda.sundayFirst : false
  readonly property bool weekNumbers: agenda ? agenda.weekNumbers : false
  readonly property var workingHours: agenda ? agenda.workingHours : null
  // { label, configured } by source key
  readonly property var sources: {
    var out = { google: { label: "Work", configured: true }, icloud: { label: "Personal", configured: true } }
    var s = agenda ? agenda.sources : []
    for (var i = 0; i < s.length; i++) out[s[i].key] = { label: s[i].label, configured: s[i].configured }
    return out
  }

  property color googleColor: Color.accent
  property color icloudColor: Color.urgent
  function sourceColor(s) { return s === "icloud" ? icloudColor : googleColor }

  // The bindings per mode (action → key specs), and the other way round
  readonly property var keyTables: L.windowKeys(agenda ? agenda.keys : null)
  readonly property var keyIndex: L.keyIndex(keyTables)

  readonly property var day: L.dayOf(byDate, selected)
  readonly property bool isToday: selected === today
  readonly property var dayGaps: L.gaps(day, selected, today, now, workingHours)
  readonly property var overlaps: L.overlapping(day)

  function events(source) { return day[source] || [] }

  readonly property var selectedEvent: {
    if (mode !== "event") return null
    var list = events(selSource)
    return selIndex >= 0 && selIndex < list.length ? { source: selSource, event: list[selIndex] } : null
  }

  // ------------------------------------------------------------ data

  function rangeFrom() { return L.gridStart(month, sundayFirst) }

  function load() {
    if (agendaProc.running) { reloadAgain = true; return }
    agendaProc.command = [command, "--agenda", "--from", rangeFrom(), "--days", "57"]
    agendaProc.running = true
  }
  property bool reloadAgain: false

  function absorb(text) {
    var data
    try { data = JSON.parse(text) } catch (e) { return }
    if (!data || !data.days) return
    var map = {}
    for (var i = 0; i < data.days.length; i++) map[data.days[i].date] = data.days[i]
    agenda = data
    byDate = map
    if (pendingEnter) { pendingEnter = false; enterEvents(true) }
    else clampSelection()
  }

  // Months the shell plugin doesn't keep fresh (it does this one and the
  // next) are fetched once when you look at them
  function ensureMonth() {
    var thisMonth = L.firstOfMonth(today)
    if (month === thisMonth || month === L.addMonths(thisMonth, 1)) return
    if (refreshedMonths[month]) return
    var r = refreshedMonths; r[month] = true; refreshedMonths = r
    refreshMonth(month)
  }

  function refreshMonth(m) {
    if (refreshProc.running) return
    refreshProc.command = [command, "--refresh", "--month", m.slice(0, 7)]
    refreshProc.running = true
  }

  function monthShown() { load(); ensureMonth() }
  onMonthChanged: monthShown()

  Process {
    id: agendaProc
    stdout: StdioCollector { waitForEnd: true; onStreamFinished: root.absorb(text) }
    onExited: if (root.reloadAgain) { root.reloadAgain = false; root.load() }
  }

  Process {
    id: refreshProc
    stderr: StdioCollector {
      waitForEnd: true
      onStreamFinished: { var t = text.trim(); if (t) root.setError(t.split("\n")[0]) }
    }
    onExited: root.load()
  }

  Process {
    id: actionProc
    property string done: ""
    stderr: StdioCollector {
      waitForEnd: true
      onStreamFinished: { var t = text.trim(); if (t) root.setError(t.split("\n").pop()) }
    }
    onExited: function(code) {
      if (code === 0) root.setStatus(done)
      root.load()
    }
  }

  function tick() {
    var d = new Date()
    today = L.fromDate(d)
    now = d.getHours() * 60 + d.getMinutes()
  }
  // Just after the minute turns
  function untilNextMinute() { return 60000 - (Date.now() % 60000) + 500 }

  // The minute ticks: countdowns, "free now", fading, and a reload (the
  // plugin's service refreshes the cache every few minutes). startOn() does
  // the first tick and load.
  Timer {
    id: minuteTimer
    running: window.visible
    onTriggered: {
      root.tick()
      root.load()
      interval = root.untilNextMinute()
      restart()
    }
  }

  Timer { id: statusTimer; interval: 3000; onTriggered: if (!root.statusError) root.status = "" }

  function setStatus(t) { status = t; statusError = false; statusTimer.restart() }
  function setError(t) { status = t; statusError = true }

  FileView {
    id: colors
    path: Color.currentThemePath + "/colors.toml"
    onLoaded: {
      var text = colors.text()
      var hex = function(key) {
        var m = new RegExp("^\\s*" + key + "\\s*=\\s*\"(#[0-9a-fA-F]{6})", "m").exec(text)
        return m ? m[1] : ""
      }
      root.googleColor = hex("blue") || Color.accent
      root.icloudColor = hex("magenta") || Color.urgent
    }
  }
  Connections {
    target: Color
    function onAccentChanged() { colors.reload() }
    function onBackgroundChanged() { colors.reload() }
  }

  // ------------------------------------------------------------ navigation (app.rs)

  function startOn(date) {
    tick()
    status = ""
    pending = null
    showHelp = false
    mode = "day"
    pendingEnter = true
    var shown = month
    selectDate(date)
    if (month === shown) monthShown() // else onMonthChanged has
    minuteTimer.interval = untilNextMinute()
    minuteTimer.restart()
  }
  // Enter event mode once the first agenda arrives, as the TUI starts
  property bool pendingEnter: false

  function selectDate(date) {
    selected = date
    if (!L.sameMonth(date, month)) month = L.firstOfMonth(date)
  }
  function selectDay(date) { selectDate(date); mode = "day" }
  function selectEvent(source, index) { mode = "event"; selSource = source; selIndex = index }
  // Follow a selection { date, source, index } from the L.* navigation
  function follow(s) {
    if (s.date !== selected) selectDate(s.date)
    selectEvent(s.source, s.index)
  }

  function clampSelection() {
    if (mode !== "event") return
    var s = L.clampSelection(byDate, selected, selSource, selIndex)
    if (s) follow(s)
    else mode = "day"
  }

  function enterEvents(quiet) {
    var s = L.enterEvents(byDate, selected, today, now)
    if (s) { follow(s); return }
    if (!quiet) setStatus("No events on this day")
    mode = "day"
  }

  function exitEvents() { mode = "day"; selSource = "google"; selIndex = 0 }

  function nextEvent() {
    var s = L.nextEvent(byDate, selected, selSource, selIndex)
    if (s) follow(s)
    return !!s
  }

  function prevEvent() {
    var s = L.prevEvent(byDate, selected, selSource, selIndex)
    if (s) follow(s)
    return !!s
  }

  function jumpEvents(n) {
    for (var k = 0; k < Math.abs(n); k++) if (!(n > 0 ? nextEvent() : prevEvent())) break
    setStatus("Jumped to " + L.dayTitle(selected))
  }

  function switchPanel() {
    var s = L.switchPanel(byDate, selected, selSource, selIndex)
    if (s) follow(s)
    else setStatus("Nothing in " + sources[L.otherSource(selSource)].label + " on this day")
  }

  function moveDays(n) { selectDate(L.addDays(selected, n)) }
  function moveMonths(n) { selectDay(L.addMonths(selected, n)) }
  function gotoToday() { selectDate(L.fromDate(new Date())) }

  // ------------------------------------------------------------ actions

  function join() {
    var e = selectedEvent
    if (!e || !e.event.joinUrl) return
    Quickshell.execDetached(["xdg-open", e.event.joinUrl])
    requestClose()
  }

  function requestAction(verb) {
    var e = selectedEvent
    if (!e) return
    var google = e.source === "google"
    if ((verb === "accept" || verb === "decline") && !google) {
      setStatus(L.VERBS[verb].name + " not supported for iCloud"); return
    }
    if (google && !sources.google.configured) { setStatus("Not signed in to Google — press S for setup"); return }
    if (!google && !sources.icloud.configured) { setStatus("iCloud not configured — press S for setup"); return }
    pending = { verb: verb, id: e.event.id, prompt: L.VERBS[verb].prompt }
  }

  function confirmAction() {
    var p = pending
    pending = null
    if (!p || actionProc.running) return
    setStatus(L.VERBS[p.verb].doing)
    statusTimer.stop()
    actionProc.done = L.VERBS[p.verb].done
    actionProc.command = [command, "--event", p.verb, p.id]
    actionProc.running = true
  }

  function openTui() {
    var argv = ["omarchy-launch-or-focus-tui", command, "--date", selected]
    Quickshell.execDetached(argv)
  }

  // ------------------------------------------------------------ keys (keymap.rs)

  function handleKey(event) {
    if (showHelp) { showHelp = false; return true }
    // The confirm prompt's keys are fixed, as the TUI's
    if (pending) {
      var t = L.latin(event.text || "")
      var key = event.key
      if (t === "y" || t === "Y" || key === Qt.Key_Return || key === Qt.Key_Enter) confirmAction()
      else if (t === "n" || t === "N" || key === Qt.Key_Escape) { pending = null; setStatus("Cancelled") }
      return true
    }
    if (statusError) { status = ""; statusError = false }
    var action = L.actionFor(keyIndex, mode, event.key, event.text, event.modifiers)
    return !!action && perform(action)
  }

  // An action, by its config name (as update.rs's apply); the ones the
  // window can't do itself open the TUI on the selected day. False if it
  // isn't one
  function perform(action) {
    switch (action) {
    case "next_day": moveDays(1); break
    case "prev_day": moveDays(-1); break
    case "next_week": moveDays(7); break
    case "prev_week": moveDays(-7); break
    case "next_month": moveMonths(1); break
    case "prev_month": moveMonths(-1); break
    case "enter_events": enterEvents(false); break
    case "next_event": if (mode === "event") nextEvent(); break
    case "prev_event": if (mode === "event") prevEvent(); break
    case "jump_events_forward": if (mode === "event") jumpEvents(10); break
    case "jump_events_back": if (mode === "event") jumpEvents(-10); break
    case "exit_events": exitEvents(); break
    case "switch_panel": if (mode === "event") switchPanel(); break
    case "join": join(); break
    case "accept": case "decline": case "delete": requestAction(action); break
    case "today": gotoToday(); break
    case "now": gotoToday(); mode = "day"; enterEvents(false); break
    case "refresh": setStatus("Refreshing..."); refreshMonth(month); break
    case "help": showHelp = true; break
    case "quit": requestClose(); break
    case "open_google_web": Quickshell.execDetached(["xdg-open", "https://calendar.google.com"]); break
    case "open_icloud_web": Quickshell.execDetached(["xdg-open", "https://www.icloud.com/calendar"]); break
    case "search": case "setup": case "toggle_logs": case "connect_google": case "connect_icloud": openTui(); break
    default: return false // an action this window doesn't know (a newer binary's)
    }
    return true
  }

  // ------------------------------------------------------------ window

  FloatingWindow {
    id: window
    title: "Calendarchy"
    color: root.bg
    implicitWidth: 1500
    implicitHeight: 900
    minimumSize: Qt.size(900, 560)

    onVisibleChanged: {
      if (!visible && root.shell && typeof root.shell.hide === "function")
        root.shell.hide(root.pluginId)
    }

    FontMetrics { id: fm; font.family: root.font; font.pixelSize: root.fs }
    readonly property real ch: fm.averageCharacterWidth

    Item {
      id: keys
      anchors.fill: parent
      focus: true
      Keys.onPressed: function(event) { if (root.handleKey(event)) event.accepted = true }

      // ---------------------------------------------------- footer
      Rectangle {
        id: footer
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: footerRow.implicitHeight + Style.space(14)
        color: "transparent"
        Rectangle { anchors.top: parent.top; width: parent.width; height: 1; color: root.faint }

        Row {
          id: footerRow
          anchors.left: parent.left
          anchors.leftMargin: Style.space(14)
          anchors.verticalCenter: parent.verticalCenter
          spacing: 0
          T {
            visible: root.status !== ""
            text: root.status
            color: root.statusError ? Color.urgent : Color.accent
          }
          Row {
            id: nextRow
            readonly property var next: L.footerNext(root.agenda ? root.agenda.next : null, root.today, root.now)
            readonly property bool soon: !!next && next.soon
            readonly property color nextColor: soon ? Color.accent : root.dim
            visible: root.status === "" && !!next
            T { text: nextRow.next ? nextRow.next.label : ""; color: nextRow.nextColor }
            T { text: nextRow.next ? nextRow.next.title : ""; color: nextRow.soon ? Color.accent : root.fg; font.bold: true }
            T { text: nextRow.next ? nextRow.next.rest : ""; color: nextRow.nextColor }
          }
        }

        T {
          anchors.right: parent.right
          anchors.rightMargin: Style.space(14)
          anchors.verticalCenter: parent.verticalCenter
          color: root.dim
          font.pixelSize: root.caption
          text: root.pending ? "y/Enter confirm · n/Esc cancel"
            : L.hints(root.mode, root.selectedEvent, root.sources, root.keyTables)
        }
      }

      // ---------------------------------------------------- calendar column
      Column {
        id: calCol
        anchors.left: parent.left
        anchors.top: parent.top
        anchors.bottom: footer.top
        anchors.margins: Style.space(14)
        width: (root.weekNumbers ? 8 : 7) * cell + Style.space(4)
        spacing: Style.space(6)
        readonly property real cell: Style.space(34)

        T {
          text: L.monthTitle(root.month) + (refreshProc.running ? " *" : "")
          font.pixelSize: Style.font.title
          font.bold: true
        }
        Item { width: 1; height: Style.space(4) }

        Grid {
          id: monthGrid
          columns: root.weekNumbers ? 8 : 7
          rowSpacing: Style.space(2)
          columnSpacing: 0
          readonly property string first: L.gridStart(root.month, root.sundayFirst)

          Repeater {
            model: root.weekNumbers ? [""].concat(L.weekdayLetters(root.sundayFirst)) : L.weekdayLetters(root.sundayFirst)
            T { required property var modelData; width: calCol.cell; horizontalAlignment: Text.AlignHCenter; text: modelData; color: root.dim; font.pixelSize: root.caption }
          }
          Repeater {
            model: 6 * (root.weekNumbers ? 8 : 7)
            Item {
              id: cellItem
              required property int index
              readonly property int cols: root.weekNumbers ? 8 : 7
              readonly property int row: Math.floor(index / cols)
              readonly property int col: index % cols - (root.weekNumbers ? 1 : 0)
              readonly property string date: L.addDays(monthGrid.first, row * 7 + Math.max(0, col))
              readonly property bool inMonth: L.sameMonth(date, root.month)
              readonly property int busy: L.busyMinutes(L.dayOf(root.byDate, date))
              readonly property bool isSel: col >= 0 && date === root.selected
              width: calCol.cell
              height: Style.space(28)

              Rectangle {
                anchors.fill: parent
                anchors.margins: 1
                radius: Style.cornerRadius
                visible: cellItem.isSel
                color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.08)
                border.width: 1
                border.color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.25)
              }
              T {
                anchors.centerIn: parent
                visible: cellItem.col < 0
                text: L.weekLabel(L.addDays(monthGrid.first, cellItem.row * 7), root.month, root.sundayFirst)
                color: root.dim
                font.pixelSize: root.caption
              }
              T {
                anchors.centerIn: parent
                visible: cellItem.col >= 0 && cellItem.inMonth
                text: String(Number(cellItem.date.slice(8)))
                color: cellItem.isSel || cellItem.date === root.today ? Color.accent
                  : cellItem.busy === 0 ? root.dim : root.fg
                font.bold: cellItem.isSel || cellItem.date === root.today || cellItem.busy >= L.HEAVY_DAY_MINUTES
              }
              MouseArea {
                anchors.fill: parent
                enabled: cellItem.col >= 0 && cellItem.inMonth
                onClicked: root.selectDay(cellItem.date)
                onDoubleClicked: { root.selectDay(cellItem.date); root.enterEvents(false) }
              }
            }
          }
        }

        Item { width: 1; height: Style.space(10) }

        // The selected week, 8–19: each day one continuous column, busy
        // stretches drawn as single blocks (darker where events overlap)
        Column {
          id: heat
          spacing: Style.space(4)
          readonly property var days: L.heatmap(root.byDate, L.weekStart(root.selected, root.sundayFirst), root.today, root.now, root.workingHours)
          readonly property real cw: Style.space(26)
          readonly property real slot: Style.space(8)
          readonly property int slots: (L.HEAT_LAST - L.HEAT_FIRST + 1) * 2
          readonly property real labelW: Style.space(22)
          readonly property real gap: Style.space(4)

          Row {
            spacing: heat.gap
            Item { width: heat.labelW; height: 1 }
            Repeater {
              model: heat.days
              T {
                required property var modelData
                width: heat.cw
                horizontalAlignment: Text.AlignHCenter
                text: L.DAY_NAMES[L.weekday(modelData.date)].charAt(0)
                color: modelData.date === root.selected || modelData.date === root.today ? Color.accent : root.dim
                font.bold: modelData.date === root.selected
                font.pixelSize: root.caption
              }
            }
          }

          Row {
            spacing: heat.gap
            Item {
              width: heat.labelW
              height: heat.slots * heat.slot
              Repeater {
                model: L.HEAT_LAST - L.HEAT_FIRST + 1
                T {
                  required property int index
                  anchors.right: parent.right
                  y: (index * 2 + 1) * heat.slot - height / 2
                  text: String(L.HEAT_FIRST + index)
                  color: root.dim
                  font.pixelSize: root.caption
                }
              }
            }
            Repeater {
              model: heat.days
              Item {
                id: dayCol
                required property var modelData
                width: heat.cw
                height: heat.slots * heat.slot
                readonly property var runs: L.heatRuns(modelData.hours)

                // Free time, lighter outside working hours
                Rectangle { anchors.fill: parent; radius: Style.cornerRadius; color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.03) }
                Repeater {
                  model: dayCol.runs.work
                  Rectangle {
                    required property var modelData
                    width: dayCol.width
                    y: modelData.from * heat.slot
                    height: (modelData.to - modelData.from) * heat.slot
                    radius: Style.cornerRadius
                    color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.04)
                  }
                }
                // A busy stretch is one block: rounded only at its ends, with
                // overlaps and the past/now change as square-edged sections
                Repeater {
                  model: dayCol.runs.busy
                  Item {
                    id: seg
                    required property var modelData
                    readonly property color base: modelData.level >= 2 ? Color.urgent : Color.accent
                    readonly property real a: (modelData.level >= 2 ? 0.75 : 0.45) * (modelData.past ? 0.45 : 1)
                    // Solid (the tint mixed into the background)
                    readonly property color fill: Qt.rgba(root.bg.r + (base.r - root.bg.r) * a, root.bg.g + (base.g - root.bg.g) * a, root.bg.b + (base.b - root.bg.b) * a, 1)
                    readonly property real r: Math.min(Style.cornerRadius, height / 2)
                    width: dayCol.width
                    y: modelData.from * heat.slot
                    height: (modelData.to - modelData.from) * heat.slot

                    Rectangle {
                      anchors.fill: parent
                      radius: seg.modelData.top || seg.modelData.bottom ? seg.r : 0
                      color: seg.fill
                    }
                    // Square off the side that continues into the next section
                    Rectangle {
                      visible: seg.modelData.top !== seg.modelData.bottom
                      width: parent.width
                      y: seg.modelData.top ? seg.r : 0
                      height: parent.height - seg.r
                      color: seg.fill
                    }
                  }
                }
              }
            }
          }

          Row {
            spacing: heat.gap
            Item { width: heat.labelW; height: 1 }
            Repeater {
              model: heat.days
              T { required property var modelData; width: heat.cw; horizontalAlignment: Text.AlignHCenter; text: modelData.marker; color: Color.accent; font.pixelSize: root.caption }
            }
          }
        }
      }

      Rectangle { id: sepL; anchors.left: calCol.right; anchors.leftMargin: Style.space(10); anchors.top: parent.top; anchors.bottom: footer.top; width: 1; color: root.faint }

      // ---------------------------------------------------- details column (event mode)
      Item {
        id: detCol
        visible: root.mode === "event"
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.bottom: footer.top
        width: visible ? Math.max(Style.space(320), Math.min(Style.space(560), (parent.width - calCol.width) * 0.4)) : 0

        Rectangle { anchors.left: parent.left; anchors.top: parent.top; anchors.bottom: parent.bottom; width: 1; color: root.faint }

        Flickable {
          anchors.fill: parent
          anchors.margins: Style.space(16)
          contentHeight: det.implicitHeight
          clip: true
          boundsBehavior: Flickable.StopAtBounds

          Column {
            id: det
            width: parent.width
            spacing: Style.space(6)
            readonly property var sel: root.selectedEvent
            readonly property var e: sel ? sel.event : null

            T { visible: !det.e; text: "No event selected"; color: root.dim }
            T {
              visible: !!det.e
              width: parent.width
              text: det.e ? det.e.title : ""
              font.pixelSize: Style.font.title
              font.bold: true
              wrapMode: Text.Wrap
              maximumLineCount: 2
              elide: Text.ElideRight
            }
            Row {
              visible: !!det.e
              T {
                text: det.e ? L.timeRange(det.e, root.twelveHour) : ""
              }
              T {
                color: root.dim
                text: !det.sel ? "" : det.sel.source === "google"
                  ? " · Google" + (det.e.calendarName ? " · " + det.e.calendarName : "")
                  : " · iCloud"
              }
            }
            T { visible: !!(det.e && det.e.location); width: parent.width; text: det.e && det.e.location ? det.e.location : ""; color: root.dim; elide: Text.ElideRight }
            T {
              visible: !!(det.e && det.e.meetingUrl)
              text: {
                if (!det.e || !det.e.meetingUrl) return ""
                var k = L.keyLabel(root.keyTables.event, "join")
                return (k ? k + " join · " : "") + L.meetingKind(det.e.meetingUrl)
              }
              color: Color.accent
              font.pixelSize: root.small
            }
            Item { width: 1; height: Style.space(6); visible: !!(det.e && det.e.description) }
            T {
              visible: !!(det.e && det.e.description)
              width: parent.width
              text: det.e ? L.descriptionText(det.e.description) : ""
              wrapMode: Text.Wrap
              font.pixelSize: root.small
              maximumLineCount: det.e && det.e.attendees.length ? 12 : 40
              elide: Text.ElideRight
            }
            Item { width: 1; height: Style.space(6); visible: !!(det.e && det.e.attendees.length) }
            T { visible: !!(det.e && det.e.attendees.length); text: det.e ? L.attendeeSummary(det.e.attendees) : ""; color: root.dim; font.pixelSize: root.small }
            Repeater {
              model: det.e ? det.e.attendees : []
              Row {
                required property var modelData
                spacing: Style.space(8)
                T {
                  width: Style.space(14)
                  text: modelData.status === "declined" ? "✗" : (modelData.status === "tentative" || modelData.status === "needsAction") ? "?" : "✓"
                  color: modelData.status === "declined" ? Color.urgent
                    : modelData.status === "accepted" ? Color.accent
                    : modelData.status === "needsAction" ? root.dim : root.fg
                  font.pixelSize: root.small
                }
                T { text: modelData.name || modelData.email; font.pixelSize: root.small }
                T { visible: modelData.status === "organizer"; text: "(org)"; color: root.dim; font.pixelSize: root.small }
              }
            }
          }
        }
      }

      // ---------------------------------------------------- events column
      Flickable {
        id: evFlick
        anchors.left: sepL.right
        anchors.right: detCol.left
        anchors.top: parent.top
        anchors.bottom: footer.top
        anchors.leftMargin: Style.space(14)
        anchors.rightMargin: Style.space(14)
        anchors.topMargin: Style.space(14)
        contentHeight: evCol.implicitHeight + Style.space(14)
        clip: true
        boundsBehavior: Flickable.StopAtBounds

        function ensureVisible(item) {
          var p = item.mapToItem(evCol, 0, 0)
          if (p.y < contentY) contentY = Math.max(0, p.y - Style.space(20))
          else if (p.y + item.height > contentY + height) contentY = Math.min(contentHeight - height, p.y + item.height - height + Style.space(20))
        }

        Column {
          id: evCol
          width: Math.min(evFlick.width, 100 * window.ch)
          spacing: Style.space(2)

          T { text: L.dayTitle(root.selected); font.pixelSize: Style.font.title; font.bold: true }
          Item { width: 1; height: Style.space(10) }

          Repeater {
            model: ["google", "icloud"]
            Column {
              id: panel
              required property string modelData
              readonly property string source: modelData
              readonly property var list: root.events(source)
              readonly property var cn: L.currentNext(list, root.isToday, root.now)
              width: evCol.width
              spacing: Style.space(1)

              Row {
                spacing: Style.space(6)
                Rectangle { width: Style.space(3); height: hdr.height - Style.space(4); anchors.verticalCenter: parent.verticalCenter; radius: 1; color: root.sourceColor(panel.source) }
                T { id: hdr; text: root.sources[panel.source].label; color: root.sourceColor(panel.source); font.bold: true }
              }
              T {
                visible: panel.list.length === 0
                leftPadding: 10 * window.ch
                width: panel.width
                elide: Text.ElideRight
                color: root.dim
                text: {
                  var n = L.nextUp(root.byDate, panel.source, root.selected, root.today, root.twelveHour)
                  return "Nothing scheduled" + (n ? " · next " + n : "")
                }
              }
              Repeater {
                model: L.panelRows(panel.list, root.dayGaps[panel.source])
                Loader {
                  required property var modelData
                  width: panel.width
                  sourceComponent: modelData.kind === "gap" ? gapRow : eventRow
                  onLoaded: {
                    item.row = modelData
                    item.source = panel.source
                    item.cn = Qt.binding(function() { return panel.cn })
                  }
                }
              }
              Item { width: 1; height: Style.space(14) }
            }
          }

          T { text: "Coming up"; color: root.dim; font.bold: true }
          Repeater {
            model: L.comingUp(root.byDate, root.selected, 5)
            Row {
              id: cu
              required property var modelData
              spacing: 0
              readonly property var fit: L.fitLine(modelData.items, Math.floor(evCol.width / window.ch) - 8, root.twelveHour)
              T { width: 8 * window.ch; text: L.shortDay(cu.modelData.date); color: L.isWeekend(cu.modelData.date) ? root.dim : root.fg; font.bold: !L.isWeekend(cu.modelData.date) }
              T { visible: cu.modelData.items.length === 0; text: "free"; color: root.dim }
              Repeater {
                model: cu.fit.parts
                Row {
                  required property var modelData
                  required property int index
                  T { visible: index > 0; text: " · "; color: root.dim }
                  T { text: modelData.time + " "; color: root.dim }
                  T { text: modelData.title }
                }
              }
              T { visible: cu.fit.more > 0; text: " +" + cu.fit.more; color: root.dim }
            }
          }
        }
      }

      // ---------------------------------------------------- confirm + help
      ConfirmDialog {
        anchors.fill: parent
        opened: !!root.pending
        message: root.pending ? root.pending.prompt : ""
        confirmText: "Yes  y/Enter"
        cancelText: "No  n/Esc"
        onConfirmed: root.confirmAction()
        onCanceled: { root.pending = null; root.setStatus("Cancelled") }
      }

      Rectangle {
        anchors.fill: parent
        visible: root.showHelp
        color: Qt.rgba(root.bg.r, root.bg.g, root.bg.b, 0.7)
        MouseArea { anchors.fill: parent; onClicked: root.showHelp = false }
        Rectangle {
          anchors.centerIn: parent
          width: helpCol.implicitWidth + Style.space(40)
          height: helpCol.implicitHeight + Style.space(36)
          color: root.bg
          radius: Style.cornerRadius
          border.width: 2
          border.color: Color.accent
          Column {
            id: helpCol
            anchors.centerIn: parent
            spacing: Style.space(3)
            T { text: "Help"; font.bold: true; font.pixelSize: Style.font.title }
            Item { width: 1; height: Style.space(6) }
            Repeater {
              model: L.helpRows(root.keyTables)
              Row {
                required property var modelData
                T { width: 16 * window.ch; text: modelData.keys; color: modelData.section ? root.dim : Color.accent; font.bold: modelData.section }
                T { text: modelData.text; color: modelData.section ? root.dim : root.fg }
              }
            }
          }
        }
      }
    }
  }

  // ---------------------------------------------------- rows

  Component {
    id: eventRow
    Item {
      id: er
      property var row: ({ event: {}, index: 0 })
      property string source: "google"
      property var cn: ({ current: -1, next: -1 })
      readonly property var e: row.event || {}
      readonly property bool selectedRow: root.mode === "event" && root.selSource === source && root.selIndex === row.index
      readonly property bool current: cn.current === row.index && e.accepted && !e.free
      readonly property bool next: cn.next === row.index && e.accepted && !e.free
      readonly property bool overlap: !!root.overlaps[e.id + "@" + e.start]
      readonly property bool past: root.selected < root.today
        || (root.isToday && !e.allDay && e.start < root.now && cn.current !== row.index)
      readonly property bool receding: past || !e.accepted
      readonly property string kind: L.meetingKind(e.meetingUrl)
      readonly property string len: L.formatLength(e)

      width: parent ? parent.width : 0
      height: Math.round(Style.space(26))
      onSelectedRowChanged: if (selectedRow) evFlick.ensureVisible(er)

      Rectangle {
        anchors.fill: parent
        radius: Style.cornerRadius
        visible: er.selectedRow || mouse.containsMouse
        color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.08)
        border.width: er.selectedRow ? 1 : 0
        border.color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.25)
      }
      MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        onClicked: root.selectEvent(er.source, er.row.index)
        onDoubleClicked: { root.selectEvent(er.source, er.row.index); root.join() }
      }

      readonly property color timeColor: selectedRow ? Color.accent
        : receding || e.free ? root.dim
        : current ? Color.accent
        : overlap ? Color.urgent
        : root.fg

      Row {
        anchors.verticalCenter: parent.verticalCenter
        anchors.left: parent.left
        anchors.right: parent.right
        T {
          width: 2 * window.ch
          horizontalAlignment: Text.AlignHCenter
          text: er.selectedRow ? "▶" : er.current ? "●" : (er.overlap && !er.receding && !er.e.free) ? "!" : er.next ? "○" : ""
          color: er.overlap && !er.selectedRow && !er.current ? Color.urgent : Color.accent
        }
        T {
          width: 8 * window.ch
          horizontalAlignment: Text.AlignRight
          text: L.timeText(er.e, root.twelveHour)
          color: er.timeColor
          font.bold: er.selectedRow || ((er.current || er.next) && !er.e.free)
        }
        Item { width: 2 * window.ch; height: 1 }
        Item {
          id: titleBox
          width: parent.width - 12 * window.ch - lenT.width - kindT.width
          height: titleT.height
          T {
            id: titleT
            width: Math.min(implicitWidth, parent.width - (locT.visible ? locT.width + window.ch : 0))
            text: er.e.title || ""
            elide: Text.ElideRight
            color: er.selectedRow ? Color.accent : er.receding || er.e.free ? root.dim : root.fg
            font.bold: er.selectedRow
          }
          T {
            id: locT
            readonly property string loc: er.e.location && er.e.location.indexOf("://") < 0 ? er.e.location : ""
            anchors.right: parent.right
            visible: loc !== "" && parent.width - titleT.implicitWidth >= 8 * window.ch
            width: Math.min(implicitWidth, 30 * window.ch)
            text: loc
            elide: Text.ElideRight
            color: root.dim
          }
        }
        T { id: lenT; width: 6 * window.ch; horizontalAlignment: Text.AlignRight; text: er.len; color: root.dim }
        T { id: kindT; width: 7 * window.ch; leftPadding: window.ch; text: er.kind; color: root.dim }
      }
    }
  }

  Component {
    id: gapRow
    Item {
      id: gr
      property var row: ({ gap: { minutes: 0, now: false } })
      property string source
      property var cn
      width: parent ? parent.width : 0
      height: Math.round(Style.space(22))
      readonly property var g: row.gap || { minutes: 0, now: false }
      readonly property color line: g.now ? Color.accent : root.faint
      Row {
        id: gapLine
        anchors.verticalCenter: parent.verticalCenter
        x: 12 * window.ch
        width: parent.width - x - 13 * window.ch
        spacing: Style.space(8)
        readonly property real side: (width - gl.implicitWidth - spacing * 2) / 2
        Rectangle { width: gapLine.side; height: 1; anchors.verticalCenter: parent.verticalCenter; color: gr.line }
        T {
          id: gl
          text: gr.g.now ? "free now · " + L.formatDuration(gr.g.minutes) : L.formatDuration(gr.g.minutes) + " free"
          color: gr.g.now ? Color.accent : root.dim
          font.pixelSize: root.small
        }
        Rectangle { width: gapLine.side; height: 1; anchors.verticalCenter: parent.verticalCenter; color: gr.line }
      }
    }
  }

  component T: Text {
    textFormat: Text.PlainText
    color: root.fg
    font.family: root.font
    font.pixelSize: root.fs
  }
}
