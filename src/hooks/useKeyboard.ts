import { useInput } from 'ink';
import { useApp, useAppDispatch, useAppState } from '../store/AppContext.js';
import type { DisplayEvent } from '../types/events.js';
import open from 'open';

interface UseKeyboardOptions {
  onQuit: () => void;
  onStartGoogleAuth: () => void;
  onStartICloudAuth: () => void;
  onRefresh: () => void;
  onAccept: (event: DisplayEvent) => void;
  onDecline: (event: DisplayEvent) => void;
  onDelete: (event: DisplayEvent) => void;
  getSelectedEvent: () => DisplayEvent | null;
}

export function useKeyboard(options: UseKeyboardOptions): void {
  const { state, dispatch } = useApp();
  const {
    onQuit,
    onStartGoogleAuth,
    onStartICloudAuth,
    onRefresh,
    onAccept,
    onDecline,
    onDelete,
    getSelectedEvent,
  } = options;

  useInput((input, key) => {
    // Handle pending action confirmation
    if (state.pendingAction) {
      if (input === 'y' || input === 'Y') {
        const event = getSelectedEvent();
        if (event) {
          switch (state.pendingAction.type) {
            case 'accept':
              onAccept(event);
              break;
            case 'decline':
              onDecline(event);
              break;
            case 'delete':
              onDelete(event);
              break;
          }
        }
        dispatch({ type: 'SET_PENDING_ACTION', action: null });
        return;
      }
      if (input === 'n' || input === 'N' || key.escape) {
        dispatch({ type: 'SET_PENDING_ACTION', action: null });
        return;
      }
      return;
    }

    // Quit
    if (key.escape || input === 'q') {
      if (state.navigationMode === 'event') {
        dispatch({ type: 'EXIT_EVENT_MODE' });
      } else {
        onQuit();
      }
      return;
    }

    // Day mode navigation
    if (state.navigationMode === 'day') {
      // j/k or down/up - navigate days
      if (input === 'j' || key.downArrow) {
        dispatch({ type: 'NEXT_DAY' });
        return;
      }
      if (input === 'k' || key.upArrow) {
        dispatch({ type: 'PREV_DAY' });
        return;
      }

      // h/l or left/right - navigate weeks
      if (input === 'h' || key.leftArrow) {
        dispatch({ type: 'PREV_WEEK' });
        return;
      }
      if (input === 'l' || key.rightArrow) {
        dispatch({ type: 'NEXT_WEEK' });
        return;
      }

      // Ctrl+d/u - navigate months
      if (key.ctrl && input === 'd') {
        dispatch({ type: 'NEXT_MONTH' });
        return;
      }
      if (key.ctrl && input === 'u') {
        dispatch({ type: 'PREV_MONTH' });
        return;
      }

      // Enter event mode
      if (key.return) {
        dispatch({ type: 'ENTER_EVENT_MODE' });
        return;
      }
    }

    // Event mode navigation
    if (state.navigationMode === 'event') {
      // j/k or down/up - navigate events
      if (input === 'j' || key.downArrow) {
        dispatch({ type: 'NEXT_EVENT' });
        return;
      }
      if (input === 'k' || key.upArrow) {
        dispatch({ type: 'PREV_EVENT' });
        return;
      }

      // Ctrl+d/u - scroll events by 10
      if (key.ctrl && input === 'd') {
        dispatch({ type: 'SCROLL_EVENTS', delta: 10 });
        return;
      }
      if (key.ctrl && input === 'u') {
        dispatch({ type: 'SCROLL_EVENTS', delta: -10 });
        return;
      }

      // h/l - navigate days in event mode
      if (input === 'h' || key.leftArrow) {
        dispatch({ type: 'PREV_DAY' });
        return;
      }
      if (input === 'l' || key.rightArrow) {
        dispatch({ type: 'NEXT_DAY' });
        return;
      }

      // J - Join meeting
      if (input === 'J') {
        const event = getSelectedEvent();
        if (event?.meetingUrl) {
          open(event.meetingUrl).catch(() => {
            dispatch({ type: 'SET_STATUS', message: 'Failed to open URL' });
          });
          dispatch({ type: 'SET_STATUS', message: 'Opening meeting...' });
        } else {
          dispatch({ type: 'SET_STATUS', message: 'No meeting URL' });
        }
        return;
      }

      // a - Accept (Google only)
      if (input === 'a') {
        const event = getSelectedEvent();
        if (event?.id.type === 'google') {
          dispatch({
            type: 'SET_PENDING_ACTION',
            action: { type: 'accept', eventTitle: event.title },
          });
        } else {
          dispatch({ type: 'SET_STATUS', message: 'Accept only available for Google events' });
        }
        return;
      }

      // d - Decline (Google only)
      if (input === 'd' && !key.ctrl) {
        const event = getSelectedEvent();
        if (event?.id.type === 'google') {
          dispatch({
            type: 'SET_PENDING_ACTION',
            action: { type: 'decline', eventTitle: event.title },
          });
        } else {
          dispatch({ type: 'SET_STATUS', message: 'Decline only available for Google events' });
        }
        return;
      }

      // x - Delete
      if (input === 'x') {
        const event = getSelectedEvent();
        if (event) {
          dispatch({
            type: 'SET_PENDING_ACTION',
            action: { type: 'delete', eventTitle: event.title },
          });
        }
        return;
      }
    }

    // Global keys
    // t - Go to today
    if (input === 't') {
      dispatch({ type: 'GOTO_TODAY' });
      return;
    }

    // n - Go to now (today + event mode)
    if (input === 'n') {
      dispatch({ type: 'GOTO_TODAY' });
      dispatch({ type: 'ENTER_EVENT_MODE' });
      return;
    }

    // r - Refresh
    if (input === 'r') {
      onRefresh();
      return;
    }

    // w - Toggle weekends
    if (input === 'w') {
      dispatch({ type: 'TOGGLE_WEEKENDS' });
      return;
    }

    // g - Start Google auth
    if (input === 'g') {
      onStartGoogleAuth();
      return;
    }

    // i - Start iCloud auth
    if (input === 'i') {
      onStartICloudAuth();
      return;
    }
  });
}
