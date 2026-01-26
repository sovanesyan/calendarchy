import { useEffect } from 'react';
import { useAppState, useAppDispatch } from '../store/AppContext.js';

/**
 * Load event cache from disk on mount
 */
export function useCache(): void {
  const state = useAppState();
  const dispatch = useAppDispatch();

  useEffect(() => {
    // Load cache from disk
    const loaded = state.events.loadFromDisk();

    if (loaded) {
      // Trigger a re-render by dispatching a status message
      dispatch({ type: 'SET_STATUS', message: 'Loaded cached events', duration: 1500 });
    }
  }, []);
}
