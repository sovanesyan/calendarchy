import type { TokenInfo } from '../../types/auth.js';
import type { DisplayEvent, DisplayAttendee, AttendeeStatus } from '../../types/events.js';
import type { CalendarEvent, EventsListResponse, CalendarMeta } from './types.js';
import { extractMeetingUrl } from '../meeting-url.js';
import { nameFromEmail } from '../../types/events.js';
import { format, parseISO } from 'date-fns';

const CALENDAR_API_BASE = 'https://www.googleapis.com/calendar/v3';

/**
 * Google Calendar API client
 */
export class CalendarClient {
  /**
   * Fetch events for a date range
   */
  async listEvents(
    token: TokenInfo,
    calendarId: string,
    timeMin: Date,
    timeMax: Date
  ): Promise<CalendarEvent[]> {
    const url = new URL(
      `${CALENDAR_API_BASE}/calendars/${encodeURIComponent(calendarId)}/events`
    );

    url.searchParams.set('timeMin', `${format(timeMin, 'yyyy-MM-dd')}T00:00:00Z`);
    url.searchParams.set('timeMax', `${format(timeMax, 'yyyy-MM-dd')}T23:59:59Z`);
    url.searchParams.set('singleEvents', 'true');
    url.searchParams.set('orderBy', 'startTime');
    url.searchParams.set('maxResults', '250');

    const allEvents: CalendarEvent[] = [];
    let pageToken: string | undefined;

    do {
      if (pageToken) {
        url.searchParams.set('pageToken', pageToken);
      }

      const response = await fetch(url.toString(), {
        headers: {
          Authorization: `Bearer ${token.accessToken}`,
        },
      });

      if (!response.ok) {
        const body = await response.text();
        throw new Error(`Calendar API error: ${body}`);
      }

      const data = (await response.json()) as EventsListResponse;
      if (data.items) {
        allEvents.push(...data.items);
      }
      pageToken = data.nextPageToken;
    } while (pageToken);

    return allEvents;
  }

  /**
   * Update the current user's response status for an event
   */
  async respondToEvent(
    token: TokenInfo,
    calendarId: string,
    eventId: string,
    responseStatus: 'accepted' | 'declined' | 'tentative'
  ): Promise<void> {
    const url = `${CALENDAR_API_BASE}/calendars/${encodeURIComponent(calendarId)}/events/${encodeURIComponent(eventId)}`;

    // First, get the current event
    const getResponse = await fetch(url, {
      headers: {
        Authorization: `Bearer ${token.accessToken}`,
      },
    });

    if (!getResponse.ok) {
      const body = await getResponse.text();
      throw new Error(`Failed to get event: ${body}`);
    }

    const event = (await getResponse.json()) as CalendarEvent;

    // Update the self attendee's response status
    if (event.attendees) {
      for (const attendee of event.attendees) {
        if (attendee.self) {
          attendee.responseStatus = responseStatus;
          break;
        }
      }
    }

    // PATCH the event back
    const patchResponse = await fetch(`${url}?sendUpdates=none`, {
      method: 'PATCH',
      headers: {
        Authorization: `Bearer ${token.accessToken}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(event),
    });

    if (!patchResponse.ok) {
      const body = await patchResponse.text();
      throw new Error(`Failed to update event: ${body}`);
    }
  }

  /**
   * Delete an event
   */
  async deleteEvent(
    token: TokenInfo,
    calendarId: string,
    eventId: string
  ): Promise<void> {
    const url = `${CALENDAR_API_BASE}/calendars/${encodeURIComponent(calendarId)}/events/${encodeURIComponent(eventId)}?sendUpdates=none`;

    const response = await fetch(url, {
      method: 'DELETE',
      headers: {
        Authorization: `Bearer ${token.accessToken}`,
      },
    });

    if (!response.ok) {
      const body = await response.text();
      throw new Error(`Failed to delete event: ${body}`);
    }
  }

  /**
   * Get calendar display name
   */
  async getCalendarName(
    token: TokenInfo,
    calendarId: string
  ): Promise<string | null> {
    const url = `${CALENDAR_API_BASE}/calendars/${encodeURIComponent(calendarId)}`;

    const response = await fetch(url, {
      headers: {
        Authorization: `Bearer ${token.accessToken}`,
      },
    });

    if (!response.ok) {
      return null;
    }

    const meta = (await response.json()) as CalendarMeta;
    return meta.summary ?? null;
  }
}

/**
 * Convert a Google Calendar event to a DisplayEvent
 */
export function googleEventToDisplay(
  event: CalendarEvent,
  calendarId: string,
  calendarName: string | null
): DisplayEvent | null {
  // Get start date
  let date: string;
  let timeStr: string;
  let endTimeStr: string | null = null;

  if (event.start.date) {
    // All-day event
    date = event.start.date;
    timeStr = 'All day';
  } else if (event.start.dateTime) {
    // Timed event - convert to local time
    const startDt = parseISO(event.start.dateTime);
    date = format(startDt, 'yyyy-MM-dd');
    timeStr = format(startDt, 'HH:mm');

    if (event.end.dateTime) {
      const endDt = parseISO(event.end.dateTime);
      endTimeStr = format(endDt, 'HH:mm');
    }
  } else {
    return null;
  }

  // Check acceptance status
  let accepted = true;
  let isOrganizer = true;

  if (event.attendees) {
    for (const attendee of event.attendees) {
      if (attendee.self) {
        accepted =
          attendee.responseStatus === 'accepted' ||
          attendee.responseStatus === 'organizer' ||
          !attendee.responseStatus;
        isOrganizer = attendee.organizer === true;
        break;
      }
    }
  }

  // Extract meeting URL
  let meetingUrl: string | null = null;

  if (event.hangoutLink) {
    meetingUrl = event.hangoutLink;
  } else if (event.conferenceData?.entryPoints) {
    for (const ep of event.conferenceData.entryPoints) {
      if (ep.entryPointType === 'video' && ep.uri) {
        meetingUrl = ep.uri;
        break;
      }
    }
  }

  if (!meetingUrl && event.location) {
    meetingUrl = extractMeetingUrl(event.location);
  }
  if (!meetingUrl && event.description) {
    meetingUrl = extractMeetingUrl(event.description);
  }

  // Convert attendees
  const attendees: DisplayAttendee[] = [];
  if (event.attendees) {
    for (const att of event.attendees) {
      if (!att.email) continue;

      let status: AttendeeStatus = 'needsAction';
      if (att.organizer) {
        status = 'organizer';
      } else {
        switch (att.responseStatus) {
          case 'accepted':
            status = 'accepted';
            break;
          case 'declined':
            status = 'declined';
            break;
          case 'tentative':
            status = 'tentative';
            break;
          default:
            status = 'needsAction';
        }
      }

      attendees.push({
        name: att.displayName ?? nameFromEmail(att.email),
        email: att.email,
        status,
      });
    }
  }

  return {
    id: {
      type: 'google',
      calendarId,
      eventId: event.id,
      calendarName,
    },
    title: event.summary ?? '(No title)',
    timeStr,
    endTimeStr,
    date,
    accepted,
    isOrganizer,
    isFree: event.transparency === 'transparent',
    meetingUrl,
    description: event.description ?? null,
    location: event.location ?? null,
    attendees,
  };
}
