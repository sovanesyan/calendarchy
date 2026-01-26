import type { AppState } from './types.js';
import type { TokenInfo, CalendarEntry } from '../types/auth.js';
import type { DisplayEvent } from '../types/events.js';
import type { EventSource, PendingAction } from '../types/navigation.js';
import { addDays, addMonths, startOfMonth, format, parseISO } from 'date-fns';

export type Action =
  // Navigation actions
  | { type: 'NEXT_DAY' }
  | { type: 'PREV_DAY' }
  | { type: 'NEXT_WEEK' }
  | { type: 'PREV_WEEK' }
  | { type: 'NEXT_MONTH' }
  | { type: 'PREV_MONTH' }
  | { type: 'GOTO_TODAY' }
  // Mode actions
  | { type: 'ENTER_EVENT_MODE' }
  | { type: 'EXIT_EVENT_MODE' }
  | { type: 'NEXT_EVENT' }
  | { type: 'PREV_EVENT' }
  | { type: 'SCROLL_EVENTS'; delta: number }
  | { type: 'SWITCH_SOURCE' }
  // Auth actions
  | { type: 'GOOGLE_DEVICE_CODE'; userCode: string; verificationUrl: string; deviceCode: string; expiresAt: string }
  | { type: 'GOOGLE_TOKEN'; tokens: TokenInfo }
  | { type: 'GOOGLE_AUTH_ERROR'; message: string }
  | { type: 'ICLOUD_DISCOVERING' }
  | { type: 'ICLOUD_DISCOVERED'; calendars: CalendarEntry[] }
  | { type: 'ICLOUD_AUTH_ERROR'; message: string }
  // Event actions
  | { type: 'GOOGLE_EVENTS'; events: DisplayEvent[]; monthDate: Date }
  | { type: 'ICLOUD_EVENTS'; events: DisplayEvent[]; monthDate: Date }
  | { type: 'CLEAR_CACHE' }
  // Loading actions
  | { type: 'SET_GOOGLE_LOADING'; loading: boolean }
  | { type: 'SET_ICLOUD_LOADING'; loading: boolean }
  | { type: 'SET_GOOGLE_NEEDS_FETCH'; needsFetch: boolean }
  | { type: 'SET_ICLOUD_NEEDS_FETCH'; needsFetch: boolean }
  // UI actions
  | { type: 'SET_STATUS'; message: string; duration?: number }
  | { type: 'CLEAR_STATUS' }
  | { type: 'SET_PENDING_ACTION'; action: PendingAction | null }
  | { type: 'TOGGLE_WEEKENDS' }
  | { type: 'SELECT_EVENT'; event: DisplayEvent | null }
  // Tick action (for status expiry)
  | { type: 'TICK' };

/**
 * Get all events for a date sorted by time
 */
function getEventsForDate(state: AppState, dateStr: string): DisplayEvent[] {
  const googleEvents = state.events.google.get(dateStr);
  const icloudEvents = state.events.icloud.get(dateStr);

  const all = [...googleEvents, ...icloudEvents];

  // Sort: All-day events first, then by time
  all.sort((a, b) => {
    const aTime = a.timeStr ?? '';
    const bTime = b.timeStr ?? '';
    if (aTime === 'All day' && bTime !== 'All day') return -1;
    if (aTime !== 'All day' && bTime === 'All day') return 1;
    return aTime.localeCompare(bTime);
  });

  return all;
}

/**
 * Find the source and index of the currently selected event
 */
function findSelectedEvent(state: AppState): { source: EventSource; index: number; event: DisplayEvent } | null {
  const events = getEventsForDate(state, state.selectedDate);
  if (events.length === 0) return null;

  const clampedIndex = Math.min(state.selectedEventIndex, events.length - 1);
  const event = events[clampedIndex];
  if (!event) return null;

  const source: EventSource = event.id.type === 'google' ? 'google' : 'icloud';
  return { source, index: clampedIndex, event };
}

export function reducer(state: AppState, action: Action): AppState {
  switch (action.type) {
    // Navigation
    case 'NEXT_DAY': {
      const newDate = addDays(parseISO(state.selectedDate), 1);
      return {
        ...state,
        selectedDate: format(newDate, 'yyyy-MM-dd'),
        selectedEventIndex: 0,
        googleNeedsFetch: true,
        icloudNeedsFetch: true,
      };
    }

    case 'PREV_DAY': {
      const newDate = addDays(parseISO(state.selectedDate), -1);
      return {
        ...state,
        selectedDate: format(newDate, 'yyyy-MM-dd'),
        selectedEventIndex: 0,
        googleNeedsFetch: true,
        icloudNeedsFetch: true,
      };
    }

    case 'NEXT_WEEK': {
      const newDate = addDays(parseISO(state.selectedDate), 7);
      return {
        ...state,
        selectedDate: format(newDate, 'yyyy-MM-dd'),
        selectedEventIndex: 0,
        googleNeedsFetch: true,
        icloudNeedsFetch: true,
      };
    }

    case 'PREV_WEEK': {
      const newDate = addDays(parseISO(state.selectedDate), -7);
      return {
        ...state,
        selectedDate: format(newDate, 'yyyy-MM-dd'),
        selectedEventIndex: 0,
        googleNeedsFetch: true,
        icloudNeedsFetch: true,
      };
    }

    case 'NEXT_MONTH': {
      const newDate = addMonths(parseISO(state.selectedDate), 1);
      return {
        ...state,
        selectedDate: format(newDate, 'yyyy-MM-dd'),
        selectedEventIndex: 0,
        googleNeedsFetch: true,
        icloudNeedsFetch: true,
      };
    }

    case 'PREV_MONTH': {
      const newDate = addMonths(parseISO(state.selectedDate), -1);
      return {
        ...state,
        selectedDate: format(newDate, 'yyyy-MM-dd'),
        selectedEventIndex: 0,
        googleNeedsFetch: true,
        icloudNeedsFetch: true,
      };
    }

    case 'GOTO_TODAY': {
      return {
        ...state,
        selectedDate: state.currentDate,
        selectedEventIndex: 0,
        googleNeedsFetch: true,
        icloudNeedsFetch: true,
      };
    }

    // Mode
    case 'ENTER_EVENT_MODE': {
      const selected = findSelectedEvent(state);
      return {
        ...state,
        navigationMode: 'event',
        selectedSource: selected?.source ?? state.selectedSource,
        selectedEvent: selected?.event ?? null,
      };
    }

    case 'EXIT_EVENT_MODE': {
      return {
        ...state,
        navigationMode: 'day',
        pendingAction: null,
        selectedEvent: null,
      };
    }

    case 'NEXT_EVENT': {
      const events = getEventsForDate(state, state.selectedDate);
      const newIndex = Math.min(state.selectedEventIndex + 1, events.length - 1);
      const event = events[newIndex] ?? null;
      return {
        ...state,
        selectedEventIndex: newIndex,
        selectedSource: event?.id.type === 'google' ? 'google' : 'icloud',
        selectedEvent: event,
      };
    }

    case 'PREV_EVENT': {
      const newIndex = Math.max(state.selectedEventIndex - 1, 0);
      const events = getEventsForDate(state, state.selectedDate);
      const event = events[newIndex] ?? null;
      return {
        ...state,
        selectedEventIndex: newIndex,
        selectedSource: event?.id.type === 'google' ? 'google' : 'icloud',
        selectedEvent: event,
      };
    }

    case 'SCROLL_EVENTS': {
      const events = getEventsForDate(state, state.selectedDate);
      const newIndex = Math.max(0, Math.min(state.selectedEventIndex + action.delta, events.length - 1));
      const event = events[newIndex] ?? null;
      return {
        ...state,
        selectedEventIndex: newIndex,
        selectedSource: event?.id.type === 'google' ? 'google' : 'icloud',
        selectedEvent: event,
      };
    }

    case 'SWITCH_SOURCE': {
      return {
        ...state,
        selectedSource: state.selectedSource === 'google' ? 'icloud' : 'google',
      };
    }

    // Auth
    case 'GOOGLE_DEVICE_CODE': {
      return {
        ...state,
        googleAuth: {
          type: 'awaitingUserCode',
          userCode: action.userCode,
          verificationUrl: action.verificationUrl,
          deviceCode: action.deviceCode,
          expiresAt: action.expiresAt,
        },
      };
    }

    case 'GOOGLE_TOKEN': {
      return {
        ...state,
        googleAuth: { type: 'authenticated', tokens: action.tokens },
        googleNeedsFetch: true,
        googleLoading: false,
      };
    }

    case 'GOOGLE_AUTH_ERROR': {
      return {
        ...state,
        googleAuth: { type: 'error', message: action.message },
        googleLoading: false,
      };
    }

    case 'ICLOUD_DISCOVERING': {
      return {
        ...state,
        icloudAuth: { type: 'discovering' },
        icloudLoading: true,
      };
    }

    case 'ICLOUD_DISCOVERED': {
      return {
        ...state,
        icloudAuth: { type: 'authenticated', calendars: action.calendars },
        icloudNeedsFetch: true,
        icloudLoading: false,
      };
    }

    case 'ICLOUD_AUTH_ERROR': {
      return {
        ...state,
        icloudAuth: { type: 'error', message: action.message },
        icloudLoading: false,
      };
    }

    // Events
    case 'GOOGLE_EVENTS': {
      state.events.google.store(action.events, action.monthDate);
      state.events.saveToDisk();
      return {
        ...state,
        googleLoading: false,
      };
    }

    case 'ICLOUD_EVENTS': {
      state.events.icloud.store(action.events, action.monthDate);
      state.events.saveToDisk();
      return {
        ...state,
        icloudLoading: false,
      };
    }

    case 'CLEAR_CACHE': {
      state.events.clear();
      return {
        ...state,
        googleNeedsFetch: true,
        icloudNeedsFetch: true,
      };
    }

    // Loading
    case 'SET_GOOGLE_LOADING': {
      return { ...state, googleLoading: action.loading };
    }

    case 'SET_ICLOUD_LOADING': {
      return { ...state, icloudLoading: action.loading };
    }

    case 'SET_GOOGLE_NEEDS_FETCH': {
      return { ...state, googleNeedsFetch: action.needsFetch };
    }

    case 'SET_ICLOUD_NEEDS_FETCH': {
      return { ...state, icloudNeedsFetch: action.needsFetch };
    }

    // UI
    case 'SET_STATUS': {
      const duration = action.duration ?? 3000;
      return {
        ...state,
        statusMessage: action.message,
        statusExpiresAt: Date.now() + duration,
      };
    }

    case 'CLEAR_STATUS': {
      return {
        ...state,
        statusMessage: null,
        statusExpiresAt: null,
      };
    }

    case 'SET_PENDING_ACTION': {
      return {
        ...state,
        pendingAction: action.action,
      };
    }

    case 'TOGGLE_WEEKENDS': {
      return {
        ...state,
        showWeekends: !state.showWeekends,
      };
    }

    case 'SELECT_EVENT': {
      return {
        ...state,
        selectedEvent: action.event,
      };
    }

    case 'TICK': {
      if (state.statusExpiresAt && Date.now() >= state.statusExpiresAt) {
        return {
          ...state,
          statusMessage: null,
          statusExpiresAt: null,
        };
      }
      return state;
    }

    default:
      return state;
  }
}
