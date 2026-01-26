import type { AttendeeStatus } from '../../types/events.js';

/**
 * Attendee from iCal ATTENDEE line
 */
export interface ICalAttendee {
  name: string | null;
  email: string;
  partstat: string; // ACCEPTED, DECLINED, TENTATIVE, NEEDS-ACTION
  isOrganizer: boolean;
}

/**
 * Event time - can be all-day (date only) or specific time
 */
export type EventTime =
  | { type: 'date'; date: string } // YYYY-MM-DD
  | { type: 'dateTime'; dateTime: Date };

/**
 * An event from iCloud Calendar (parsed from iCal/VCALENDAR format)
 */
export interface ICalEvent {
  uid: string;
  summary: string | null;
  dtstart: EventTime;
  dtend: EventTime | null;
  location: string | null;
  description: string | null;
  url: string | null;
  accepted: boolean;
  attendees: ICalAttendee[];
  transp: string | null; // "TRANSPARENT" = free, "OPAQUE" = busy
  calendarUrl: string;
  etag: string | null;
}

/**
 * Information about a calendar
 */
export interface CalendarInfo {
  url: string;
  name: string | null;
}

/**
 * Map iCal PARTSTAT to AttendeeStatus
 */
export function mapPartstatToStatus(partstat: string, isOrganizer: boolean): AttendeeStatus {
  if (isOrganizer) return 'organizer';

  switch (partstat.toUpperCase()) {
    case 'ACCEPTED':
      return 'accepted';
    case 'DECLINED':
      return 'declined';
    case 'TENTATIVE':
      return 'tentative';
    default:
      return 'needsAction';
  }
}
