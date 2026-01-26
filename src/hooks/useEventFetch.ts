import { useEffect, useCallback } from 'react';
import { useAppState, useAppDispatch } from '../store/AppContext.js';
import { CalendarClient, googleEventToDisplay } from '../services/google/calendar.js';
import { ICloudAuth } from '../services/icloud/auth.js';
import { CalDavClient } from '../services/icloud/caldav.js';
import { icloudEventToDisplay } from '../services/icloud/conversion.js';
import { startOfMonth, endOfMonth, parseISO } from 'date-fns';
import type { DisplayEvent } from '../types/events.js';

export function useEventFetch(): {
  refresh: () => void;
} {
  const state = useAppState();
  const dispatch = useAppDispatch();

  // Fetch Google events
  useEffect(() => {
    if (!state.googleNeedsFetch) return;
    if (state.googleAuth.type !== 'authenticated') return;

    const selectedDate = parseISO(state.selectedDate);
    const monthStart = startOfMonth(selectedDate);

    // Check if already fetched
    if (state.events.google.hasMonth(monthStart)) {
      dispatch({ type: 'SET_GOOGLE_NEEDS_FETCH', needsFetch: false });
      return;
    }

    // Capture tokens before async boundary (TypeScript narrowing doesn't work in closures)
    const tokens = state.googleAuth.tokens;
    const calendarId = state.config.google?.calendarId ?? 'primary';

    dispatch({ type: 'SET_GOOGLE_LOADING', loading: true });
    dispatch({ type: 'SET_GOOGLE_NEEDS_FETCH', needsFetch: false });

    const fetchGoogle = async () => {
      try {
        const client = new CalendarClient();
        const monthEnd = endOfMonth(monthStart);

        // Get calendar name
        const calendarName = await client.getCalendarName(tokens, calendarId);

        // Fetch events
        const events = await client.listEvents(tokens, calendarId, monthStart, monthEnd);

        // Convert to display events
        const displayEvents: DisplayEvent[] = [];
        for (const event of events) {
          const display = googleEventToDisplay(event, calendarId, calendarName);
          if (display) {
            displayEvents.push(display);
          }
        }

        dispatch({ type: 'GOOGLE_EVENTS', events: displayEvents, monthDate: monthStart });
      } catch (error) {
        dispatch({ type: 'SET_STATUS', message: `Google fetch error: ${error}` });
        dispatch({ type: 'SET_GOOGLE_LOADING', loading: false });
      }
    };

    fetchGoogle();
  }, [state.googleNeedsFetch, state.googleAuth.type, state.selectedDate]);

  // Fetch iCloud events
  useEffect(() => {
    if (!state.icloudNeedsFetch) return;
    if (state.icloudAuth.type !== 'authenticated') return;
    if (!state.config.icloud) return;

    const selectedDate = parseISO(state.selectedDate);
    const monthStart = startOfMonth(selectedDate);

    // Check if already fetched
    if (state.events.icloud.hasMonth(monthStart)) {
      dispatch({ type: 'SET_ICLOUD_NEEDS_FETCH', needsFetch: false });
      return;
    }

    // Capture values before async boundary
    const icloudConfig = state.config.icloud;
    const calendars = state.icloudAuth.calendars;

    dispatch({ type: 'SET_ICLOUD_LOADING', loading: true });
    dispatch({ type: 'SET_ICLOUD_NEEDS_FETCH', needsFetch: false });

    const fetchICloud = async () => {
      try {
        const auth = new ICloudAuth(icloudConfig);
        const client = new CalDavClient(auth);
        const monthEnd = endOfMonth(monthStart);

        const allEvents: DisplayEvent[] = [];

        // Fetch from all calendars
        for (const calendar of calendars) {
          try {
            const events = await client.fetchEvents(calendar.url, monthStart, monthEnd);

            for (const event of events) {
              const display = icloudEventToDisplay(event, calendar.name);
              allEvents.push(display);
            }
          } catch {
            // Continue with other calendars on error
          }
        }

        dispatch({ type: 'ICLOUD_EVENTS', events: allEvents, monthDate: monthStart });
      } catch (error) {
        dispatch({ type: 'SET_STATUS', message: `iCloud fetch error: ${error}` });
        dispatch({ type: 'SET_ICLOUD_LOADING', loading: false });
      }
    };

    fetchICloud();
  }, [state.icloudNeedsFetch, state.icloudAuth.type, state.selectedDate]);

  const refresh = useCallback(() => {
    dispatch({ type: 'CLEAR_CACHE' });
    dispatch({ type: 'SET_STATUS', message: 'Refreshing...' });
  }, [dispatch]);

  return { refresh };
}
