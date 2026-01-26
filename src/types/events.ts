/**
 * Attendee response status
 */
export type AttendeeStatus =
  | 'accepted'
  | 'declined'
  | 'tentative'
  | 'needsAction'
  | 'organizer';

/**
 * Attendee information for display
 */
export interface DisplayAttendee {
  name: string | null;
  email: string;
  status: AttendeeStatus;
}

/**
 * Event identifier for API actions (accept/decline/delete)
 */
export type EventId =
  | {
      type: 'google';
      calendarId: string;
      eventId: string;
      calendarName: string | null;
    }
  | {
      type: 'icloud';
      calendarUrl: string;
      eventUid: string;
      etag: string | null;
      calendarName: string | null;
    };

/**
 * Unified event representation for display
 */
export interface DisplayEvent {
  id: EventId;
  title: string;
  timeStr: string;
  endTimeStr: string | null;
  date: string; // ISO date string YYYY-MM-DD
  accepted: boolean;
  isOrganizer: boolean;
  isFree: boolean;
  meetingUrl: string | null;
  description: string | null;
  location: string | null;
  attendees: DisplayAttendee[];
}

/**
 * Get the display icon for an attendee status
 */
export function getStatusIcon(status: AttendeeStatus): string {
  switch (status) {
    case 'accepted':
    case 'organizer':
      return '\u2713'; // ✓
    case 'declined':
      return '\u2717'; // ✗
    case 'tentative':
    case 'needsAction':
      return '?';
  }
}

/**
 * Get the sort order for attendee status (lower = first)
 */
export function getStatusSortOrder(status: AttendeeStatus): number {
  switch (status) {
    case 'organizer':
      return 0;
    case 'accepted':
      return 1;
    case 'tentative':
      return 2;
    case 'needsAction':
      return 3;
    case 'declined':
      return 4;
  }
}

/**
 * Sort attendees by status (accepted first, declined last), then by name
 */
export function sortAttendees(attendees: DisplayAttendee[]): DisplayAttendee[] {
  return [...attendees].sort((a, b) => {
    const statusCmp = getStatusSortOrder(a.status) - getStatusSortOrder(b.status);
    if (statusCmp !== 0) return statusCmp;
    return (a.name ?? '').localeCompare(b.name ?? '');
  });
}

/**
 * Extract a display name from an email address
 * e.g., "john.smith@example.com" -> "John Smith"
 */
export function nameFromEmail(email: string): string {
  const local = email.split('@')[0] ?? email;
  const parts = local.split(/[._-]/);
  return parts
    .map((p) => (p.charAt(0).toUpperCase() + p.slice(1)))
    .join(' ');
}
