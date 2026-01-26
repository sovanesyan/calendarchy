/**
 * Event source (which calendar provider)
 */
export type EventSource = 'google' | 'icloud';

/**
 * Navigation mode
 */
export type NavigationMode = 'day' | 'event';

/**
 * Pending action type for confirmation modal
 */
export type PendingActionType = 'accept' | 'decline' | 'delete';

/**
 * Pending action requiring confirmation
 */
export interface PendingAction {
  type: PendingActionType;
  eventTitle: string;
}
