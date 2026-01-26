import { useMemo } from 'react';
import { useAppState } from '../store/AppContext.js';
import type { DisplayEvent } from '../types/events.js';

/**
 * Get events for the selected date, sorted by time
 */
export function useSelectedDateEvents(): DisplayEvent[] {
  const state = useAppState();

  return useMemo(() => {
    const googleEvents = state.events.google.get(state.selectedDate);
    const icloudEvents = state.events.icloud.get(state.selectedDate);

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
  }, [state.events, state.selectedDate]);
}

/**
 * Get the currently selected event (in event mode)
 */
export function useSelectedEvent(): DisplayEvent | null {
  const state = useAppState();
  const events = useSelectedDateEvents();

  if (state.navigationMode !== 'event') {
    return null;
  }

  if (events.length === 0) {
    return null;
  }

  const index = Math.min(state.selectedEventIndex, events.length - 1);
  return events[index] ?? null;
}
