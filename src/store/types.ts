import type { Config } from '../config/loader.js';
import type { GoogleAuthState, ICloudAuthState } from '../types/auth.js';
import type { DisplayEvent } from '../types/events.js';
import type { EventSource, NavigationMode, PendingAction } from '../types/navigation.js';
import { EventCache } from '../cache/event-cache.js';

/**
 * Application state
 */
export interface AppState {
  // Config
  config: Config;

  // Auth states
  googleAuth: GoogleAuthState;
  icloudAuth: ICloudAuthState;

  // Navigation
  currentDate: string; // Today's date YYYY-MM-DD
  selectedDate: string; // Currently selected date YYYY-MM-DD
  navigationMode: NavigationMode;
  selectedSource: EventSource;
  selectedEventIndex: number;

  // Events
  events: EventCache;

  // Loading states
  googleLoading: boolean;
  icloudLoading: boolean;
  googleNeedsFetch: boolean;
  icloudNeedsFetch: boolean;

  // UI state
  statusMessage: string | null;
  statusExpiresAt: number | null;
  pendingAction: PendingAction | null;
  showWeekends: boolean;

  // Current event for event mode
  selectedEvent: DisplayEvent | null;
}

/**
 * Create initial state
 */
export function createInitialState(config: Config): AppState {
  const today = new Date().toISOString().slice(0, 10);

  return {
    config,
    googleAuth: config.google ? { type: 'notAuthenticated' } : { type: 'notConfigured' },
    icloudAuth: config.icloud ? { type: 'notAuthenticated' } : { type: 'notConfigured' },
    currentDate: today,
    selectedDate: today,
    navigationMode: 'day',
    selectedSource: 'google',
    selectedEventIndex: 0,
    events: new EventCache(),
    googleLoading: false,
    icloudLoading: false,
    googleNeedsFetch: false,
    icloudNeedsFetch: false,
    statusMessage: null,
    statusExpiresAt: null,
    pendingAction: null,
    showWeekends: true,
    selectedEvent: null,
  };
}
