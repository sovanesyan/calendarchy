import type { DisplayEvent, DisplayAttendee } from '../../types/events.js';
import type { ICalEvent } from './types.js';
import { mapPartstatToStatus } from './types.js';
import { extractMeetingUrl, isMeetingUrl } from '../meeting-url.js';
import { nameFromEmail } from '../../types/events.js';
import { format } from 'date-fns';

/**
 * Convert an iCloud Calendar event to a DisplayEvent
 */
export function icloudEventToDisplay(
  event: ICalEvent,
  calendarName: string | null
): DisplayEvent {
  // Get date and time strings
  let date: string;
  let timeStr: string;
  let endTimeStr: string | null = null;

  if (event.dtstart.type === 'date') {
    date = event.dtstart.date;
    timeStr = 'All day';
  } else {
    date = format(event.dtstart.dateTime, 'yyyy-MM-dd');
    timeStr = format(event.dtstart.dateTime, 'HH:mm');

    if (event.dtend?.type === 'dateTime') {
      endTimeStr = format(event.dtend.dateTime, 'HH:mm');
    }
  }

  // Check if user is organizer
  let isOrganizer = false;
  for (const att of event.attendees) {
    if (att.isOrganizer) {
      isOrganizer = true;
      break;
    }
  }

  // Extract meeting URL
  let meetingUrl: string | null = null;

  if (event.url && isMeetingUrl(event.url)) {
    meetingUrl = event.url;
  }
  if (!meetingUrl && event.location) {
    meetingUrl = extractMeetingUrl(event.location);
  }
  if (!meetingUrl && event.description) {
    meetingUrl = extractMeetingUrl(event.description);
  }

  // Convert attendees
  const attendees: DisplayAttendee[] = event.attendees.map((att) => ({
    name: att.name ?? nameFromEmail(att.email),
    email: att.email,
    status: mapPartstatToStatus(att.partstat, att.isOrganizer),
  }));

  return {
    id: {
      type: 'icloud',
      calendarUrl: event.calendarUrl,
      eventUid: event.uid,
      etag: event.etag,
      calendarName,
    },
    title: event.summary ?? '(No title)',
    timeStr,
    endTimeStr,
    date,
    accepted: event.accepted,
    isOrganizer,
    isFree: event.transp === 'TRANSPARENT',
    meetingUrl,
    description: event.description,
    location: event.location,
    attendees,
  };
}
