import { useEffect, useRef, useCallback } from 'react';
import { useAppState, useAppDispatch } from '../store/AppContext.js';
import { GoogleAuth } from '../services/google/auth.js';
import { saveGoogleTokens, loadGoogleTokens } from '../config/tokens.js';
import { isTokenExpired } from '../types/auth.js';

const POLL_INTERVAL = 5000; // 5 seconds

export function useGoogleAuth(): {
  startAuth: () => void;
  isAuthenticated: boolean;
} {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const pollIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const authRef = useRef<GoogleAuth | null>(null);

  // Initialize auth client
  if (!authRef.current && state.config.google) {
    authRef.current = new GoogleAuth(state.config.google);
  }

  // Try to load saved tokens on mount
  useEffect(() => {
    if (state.googleAuth.type !== 'notAuthenticated') return;
    if (!state.config.google) return;

    const tokens = loadGoogleTokens();
    if (!tokens) return;

    if (!isTokenExpired(tokens)) {
      dispatch({ type: 'GOOGLE_TOKEN', tokens });
    } else if (tokens.refreshToken) {
      // Token expired but we have a refresh token
      dispatch({ type: 'SET_GOOGLE_LOADING', loading: true });

      const auth = new GoogleAuth(state.config.google);
      auth
        .refreshToken(tokens.refreshToken)
        .then((newTokens) => {
          saveGoogleTokens(newTokens);
          dispatch({ type: 'GOOGLE_TOKEN', tokens: newTokens });
        })
        .catch((error) => {
          dispatch({ type: 'GOOGLE_AUTH_ERROR', message: String(error) });
        });
    }
  }, []);

  // Poll for token when awaiting user code
  useEffect(() => {
    if (state.googleAuth.type !== 'awaitingUserCode') {
      if (pollIntervalRef.current) {
        clearInterval(pollIntervalRef.current);
        pollIntervalRef.current = null;
      }
      return;
    }

    const auth = authRef.current;
    if (!auth) return;

    const { deviceCode, expiresAt } = state.googleAuth;

    const poll = async () => {
      // Check if expired
      if (new Date() >= new Date(expiresAt)) {
        if (pollIntervalRef.current) {
          clearInterval(pollIntervalRef.current);
          pollIntervalRef.current = null;
        }
        dispatch({ type: 'GOOGLE_AUTH_ERROR', message: 'Authorization expired' });
        return;
      }

      try {
        const result = await auth.pollForToken(deviceCode);

        switch (result.type) {
          case 'success':
            if (pollIntervalRef.current) {
              clearInterval(pollIntervalRef.current);
              pollIntervalRef.current = null;
            }
            saveGoogleTokens(result.tokens);
            dispatch({ type: 'GOOGLE_TOKEN', tokens: result.tokens });
            dispatch({ type: 'SET_STATUS', message: 'Google authenticated!' });
            break;
          case 'denied':
            if (pollIntervalRef.current) {
              clearInterval(pollIntervalRef.current);
              pollIntervalRef.current = null;
            }
            dispatch({ type: 'GOOGLE_AUTH_ERROR', message: 'Authorization denied' });
            break;
          case 'expired':
            if (pollIntervalRef.current) {
              clearInterval(pollIntervalRef.current);
              pollIntervalRef.current = null;
            }
            dispatch({ type: 'GOOGLE_AUTH_ERROR', message: 'Authorization expired' });
            break;
          // 'pending' and 'slowDown' - continue polling
        }
      } catch (error) {
        // Continue polling on network errors
      }
    };

    // Start polling
    pollIntervalRef.current = setInterval(poll, POLL_INTERVAL);

    // Cleanup
    return () => {
      if (pollIntervalRef.current) {
        clearInterval(pollIntervalRef.current);
        pollIntervalRef.current = null;
      }
    };
  }, [state.googleAuth.type]);

  const startAuth = useCallback(async () => {
    const auth = authRef.current;
    if (!auth) {
      dispatch({ type: 'SET_STATUS', message: 'Google not configured' });
      return;
    }

    if (state.googleAuth.type === 'awaitingUserCode') {
      return; // Already in progress
    }

    try {
      const deviceCode = await auth.requestDeviceCode();
      const expiresAt = new Date(Date.now() + deviceCode.expires_in * 1000).toISOString();

      dispatch({
        type: 'GOOGLE_DEVICE_CODE',
        userCode: deviceCode.user_code,
        verificationUrl: deviceCode.verification_url,
        deviceCode: deviceCode.device_code,
        expiresAt,
      });
    } catch (error) {
      dispatch({ type: 'GOOGLE_AUTH_ERROR', message: String(error) });
    }
  }, [state.googleAuth.type, dispatch]);

  return {
    startAuth,
    isAuthenticated: state.googleAuth.type === 'authenticated',
  };
}
