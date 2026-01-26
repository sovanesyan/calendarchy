# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
npm install              # Install dependencies
npm run build            # Build TypeScript to dist/
npm start                # Run the built app
npm run dev              # Watch mode for development
npm run typecheck        # Type check without emitting
npm test                 # Run tests
```

## Architecture

Calendarchy is a terminal calendar app built with TypeScript and INK (React for CLIs) that displays Google Calendar and iCloud Calendar events side by side.

### Core Flow

1. **Startup**: `index.tsx` loads config, restores cached events from disk for instant display
2. **Auth**: Google uses OAuth device flow; iCloud uses app-specific password with CalDAV discovery
3. **Fetching**: Events are fetched per-month via React hooks, converted to `DisplayEvent`, cached to disk
4. **Rendering**: INK React components render a month calendar grid and event panels

### Directory Structure

```
src/
├── index.tsx              # Entry point, renders App with AppProvider
├── App.tsx                # Root component, orchestrates hooks and UI
│
├── components/            # INK UI components
│   ├── CalendarGrid.tsx   # Month calendar view with week numbers
│   ├── EventPanel.tsx     # Event list for selected date
│   ├── EventDetails.tsx   # Selected event details panel
│   ├── StatusBar.tsx      # Bottom status bar with mode/auth indicators
│   ├── ConfirmModal.tsx   # Yes/No confirmation dialog
│   └── AuthPrompt.tsx     # OAuth device code display
│
├── hooks/                 # Custom React hooks
│   ├── useKeyboard.ts     # Keyboard input handling (vim-style)
│   ├── useGoogleAuth.ts   # Google OAuth device flow
│   ├── useICloudAuth.ts   # iCloud CalDAV discovery
│   ├── useEventFetch.ts   # Background event fetching
│   ├── useCache.ts        # Load cache on mount
│   └── useNavigation.ts   # Get selected events
│
├── services/              # API clients
│   ├── google/
│   │   ├── auth.ts        # OAuth device flow
│   │   ├── calendar.ts    # Calendar API + DisplayEvent conversion
│   │   └── types.ts       # API response types
│   ├── icloud/
│   │   ├── auth.ts        # Basic auth helper
│   │   ├── caldav.ts      # CalDAV client + iCal parser
│   │   ├── types.ts       # ICalEvent type
│   │   └── conversion.ts  # ICalEvent to DisplayEvent
│   └── meeting-url.ts     # Extract Zoom/Meet/Teams URLs
│
├── store/                 # State management (React Context + useReducer)
│   ├── AppContext.tsx     # Provider and hooks (useApp, useAppState, useAppDispatch)
│   ├── reducer.ts         # Actions: navigation, auth, events, UI
│   └── types.ts           # AppState interface
│
├── config/                # Configuration
│   ├── paths.ts           # ~/.config/calendarchy, ~/.cache/calendarchy paths
│   ├── loader.ts          # Load config.json
│   └── tokens.ts          # Token storage (read/write tokens.json)
│
├── cache/
│   └── event-cache.ts     # SourceCache, EventCache, disk persistence
│
└── types/                 # Shared types
    ├── events.ts          # DisplayEvent, EventId, DisplayAttendee
    ├── auth.ts            # TokenInfo, GoogleAuthState, ICloudAuthState
    └── navigation.ts      # EventSource, NavigationMode, PendingAction
```

### Key Types

- `DisplayEvent` - Normalized event with title, timeStr, date, accepted, meetingUrl
- `GoogleAuthState` / `ICloudAuthState` - Auth state machines (discriminated unions)
- `AppState` - Full application state managed by reducer
- `Action` - Reducer action types (navigation, auth, events, UI)

### Data Flow

```
Config → Auth → Fetch Events → Convert to DisplayEvent → Store in SourceCache → Save to disk
                                                                              ↓
UI ← EventCache.get(date) ←──────────────────────────────────────────────────┘
```

### Caching

- Events cached to `~/.cache/calendarchy/events.json`
- Auth tokens stored in `~/.config/calendarchy/tokens.json`
- Cache loads on startup for instant display; `fetchedMonths` not restored to force refresh

### Keyboard Shortcuts

**Day Mode:**
- `j/k` or `↓/↑` - Navigate days
- `h/l` or `←/→` - Navigate weeks
- `Ctrl+d/u` - Navigate months
- `Enter` - Enter event mode
- `t` - Go to today
- `n` - Go to now (today + event mode)

**Event Mode:**
- `j/k` or `↓/↑` - Navigate events
- `h/l` or `←/→` - Navigate days
- `Ctrl+d/u` - Scroll events by 10
- `Esc` - Exit event mode
- `J` - Join meeting (open URL)
- `a/d` - Accept/decline (Google only)
- `x` - Delete event

**Global:**
- `r` - Refresh events
- `w` - Toggle weekends
- `g` - Start Google auth
- `i` - Start iCloud auth
- `q` - Quit
