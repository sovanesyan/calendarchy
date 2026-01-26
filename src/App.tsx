import React, { useCallback, useEffect } from 'react';
import { Box, useApp as useInkApp } from 'ink';
import { useApp, useAppDispatch, useAppState } from './store/AppContext.js';
import { CalendarGrid } from './components/CalendarGrid.js';
import { EventPanel } from './components/EventPanel.js';
import { EventDetails } from './components/EventDetails.js';
import { StatusBar } from './components/StatusBar.js';
import { ConfirmModal } from './components/ConfirmModal.js';
import { AuthPrompt } from './components/AuthPrompt.js';
import { useKeyboard } from './hooks/useKeyboard.js';
import { useGoogleAuth } from './hooks/useGoogleAuth.js';
import { useICloudAuth } from './hooks/useICloudAuth.js';
import { useEventFetch } from './hooks/useEventFetch.js';
import { useCache } from './hooks/useCache.js';
import { useSelectedEvent, useSelectedDateEvents } from './hooks/useNavigation.js';
import type { DisplayEvent } from './types/events.js';
import { CalendarClient } from './services/google/calendar.js';
import { ICloudAuth } from './services/icloud/auth.js';
import { CalDavClient } from './services/icloud/caldav.js';

export function App(): JSX.Element {
  const { exit } = useInkApp();
  const { state, dispatch } = useApp();
  const { startAuth: startGoogleAuth } = useGoogleAuth();
  const { startAuth: startICloudAuth } = useICloudAuth();
  const { refresh } = useEventFetch();
  const selectedEvent = useSelectedEvent();
  const events = useSelectedDateEvents();

  // Load cache on mount
  useCache();

  // Status expiry tick
  useEffect(() => {
    const interval = setInterval(() => {
      dispatch({ type: 'TICK' });
    }, 1000);
    return () => clearInterval(interval);
  }, [dispatch]);

  // Get selected event for keyboard handler
  const getSelectedEvent = useCallback((): DisplayEvent | null => {
    if (state.navigationMode !== 'event') return null;
    if (events.length === 0) return null;
    const index = Math.min(state.selectedEventIndex, events.length - 1);
    return events[index] ?? null;
  }, [state.navigationMode, state.selectedEventIndex, events]);

  // Event action handlers
  const handleAccept = useCallback(async (event: DisplayEvent) => {
    if (event.id.type !== 'google') return;
    if (state.googleAuth.type !== 'authenticated') return;

    dispatch({ type: 'SET_STATUS', message: 'Accepting...' });

    try {
      const client = new CalendarClient();
      await client.respondToEvent(
        state.googleAuth.tokens,
        event.id.calendarId,
        event.id.eventId,
        'accepted'
      );
      dispatch({ type: 'SET_STATUS', message: 'Accepted!' });
      refresh();
    } catch (error) {
      dispatch({ type: 'SET_STATUS', message: `Error: ${error}` });
    }
  }, [state.googleAuth, dispatch, refresh]);

  const handleDecline = useCallback(async (event: DisplayEvent) => {
    if (event.id.type !== 'google') return;
    if (state.googleAuth.type !== 'authenticated') return;

    dispatch({ type: 'SET_STATUS', message: 'Declining...' });

    try {
      const client = new CalendarClient();
      await client.respondToEvent(
        state.googleAuth.tokens,
        event.id.calendarId,
        event.id.eventId,
        'declined'
      );
      dispatch({ type: 'SET_STATUS', message: 'Declined' });
      refresh();
    } catch (error) {
      dispatch({ type: 'SET_STATUS', message: `Error: ${error}` });
    }
  }, [state.googleAuth, dispatch, refresh]);

  const handleDelete = useCallback(async (event: DisplayEvent) => {
    dispatch({ type: 'SET_STATUS', message: 'Deleting...' });

    try {
      if (event.id.type === 'google') {
        if (state.googleAuth.type !== 'authenticated') return;

        const client = new CalendarClient();
        await client.deleteEvent(
          state.googleAuth.tokens,
          event.id.calendarId,
          event.id.eventId
        );
      } else {
        if (!state.config.icloud) return;

        const auth = new ICloudAuth(state.config.icloud);
        const client = new CalDavClient(auth);
        await client.deleteEvent(
          event.id.calendarUrl,
          event.id.eventUid,
          event.id.etag
        );
      }

      dispatch({ type: 'SET_STATUS', message: 'Deleted' });
      refresh();
    } catch (error) {
      dispatch({ type: 'SET_STATUS', message: `Error: ${error}` });
    }
  }, [state.googleAuth, state.config.icloud, dispatch, refresh]);

  // Set up keyboard handling
  useKeyboard({
    onQuit: exit,
    onStartGoogleAuth: startGoogleAuth,
    onStartICloudAuth: startICloudAuth,
    onRefresh: refresh,
    onAccept: handleAccept,
    onDecline: handleDecline,
    onDelete: handleDelete,
    getSelectedEvent,
  });

  // Check for no config
  useEffect(() => {
    if (!state.config.google && !state.config.icloud) {
      dispatch({
        type: 'SET_STATUS',
        message: 'No calendars configured. Edit ~/.config/calendarchy/config.json',
        duration: 10000,
      });
    }
  }, []);

  return (
    <Box flexDirection="column" width="100%">
      {/* Main content area */}
      <Box flexGrow={1}>
        {/* Left: Calendar grid */}
        <Box width={state.showWeekends ? 26 : 22} flexShrink={0}>
          <CalendarGrid showWeekends={state.showWeekends} />
        </Box>

        {/* Center: Event list */}
        <Box flexGrow={1} marginLeft={2} flexDirection="column">
          <EventPanel maxHeight={15} />
        </Box>

        {/* Right: Event details (in event mode) */}
        {state.navigationMode === 'event' && (
          <Box width={40} marginLeft={2} flexDirection="column">
            <EventDetails event={selectedEvent} />
          </Box>
        )}
      </Box>

      {/* Auth prompt overlay */}
      {state.googleAuth.type === 'awaitingUserCode' && (
        <Box position="absolute" marginTop={2} marginLeft={30}>
          <AuthPrompt
            userCode={state.googleAuth.userCode}
            verificationUrl={state.googleAuth.verificationUrl}
          />
        </Box>
      )}

      {/* Confirmation modal overlay */}
      {state.pendingAction && (
        <Box position="absolute" marginTop={5} marginLeft={30}>
          <ConfirmModal action={state.pendingAction} />
        </Box>
      )}

      {/* Status bar at bottom */}
      <Box marginTop={1}>
        <StatusBar />
      </Box>
    </Box>
  );
}
