import { useEffect, useCallback } from 'react';
import { useAppState, useAppDispatch } from '../store/AppContext.js';
import { ICloudAuth } from '../services/icloud/auth.js';
import { CalDavClient } from '../services/icloud/caldav.js';
import { saveICloudTokens, loadICloudTokens } from '../config/tokens.js';

export function useICloudAuth(): {
  startAuth: () => void;
  isAuthenticated: boolean;
} {
  const state = useAppState();
  const dispatch = useAppDispatch();

  // Try to load saved calendars on mount
  useEffect(() => {
    if (state.icloudAuth.type !== 'notAuthenticated') return;
    if (!state.config.icloud) return;

    const calendars = loadICloudTokens();
    if (calendars && calendars.length > 0) {
      dispatch({ type: 'ICLOUD_DISCOVERED', calendars });
    }
  }, []);

  const startAuth = useCallback(async () => {
    if (!state.config.icloud) {
      dispatch({ type: 'SET_STATUS', message: 'iCloud not configured' });
      return;
    }

    if (state.icloudAuth.type === 'discovering') {
      return; // Already in progress
    }

    dispatch({ type: 'ICLOUD_DISCOVERING' });

    try {
      const auth = new ICloudAuth(state.config.icloud);
      const client = new CalDavClient(auth);

      const calendars = await client.discoverCalendars();

      if (calendars.length === 0) {
        dispatch({ type: 'ICLOUD_AUTH_ERROR', message: 'No calendars found' });
        return;
      }

      // Save discovered calendars
      saveICloudTokens(calendars);

      dispatch({ type: 'ICLOUD_DISCOVERED', calendars });
      dispatch({ type: 'SET_STATUS', message: `iCloud: ${calendars.length} calendars found` });
    } catch (error) {
      dispatch({ type: 'ICLOUD_AUTH_ERROR', message: String(error) });
    }
  }, [state.config.icloud, state.icloudAuth.type, dispatch]);

  return {
    startAuth,
    isAuthenticated: state.icloudAuth.type === 'authenticated',
  };
}
