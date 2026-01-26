import type { ICloudAuth } from './auth.js';
import type { CalendarInfo, ICalEvent, EventTime, ICalAttendee } from './types.js';
import { format } from 'date-fns';

const CALDAV_SERVER = 'https://caldav.icloud.com';

/**
 * CalDAV client for iCloud Calendar
 */
export class CalDavClient {
  constructor(private auth: ICloudAuth) {}

  /**
   * Discover the user's calendars
   */
  async discoverCalendars(): Promise<CalendarInfo[]> {
    // Step 1: Get principal URL
    const principalUrl = await this.discoverPrincipal();

    // Step 2: Get calendar home set
    const calendarHome = await this.getCalendarHome(principalUrl);

    // Step 3: List calendars
    const calendars = await this.listCalendars(calendarHome);

    return calendars;
  }

  /**
   * Fetch events for a date range
   */
  async fetchEvents(
    calendarUrl: string,
    start: Date,
    end: Date
  ): Promise<ICalEvent[]> {
    const startStr = format(start, "yyyyMMdd'T'000000'Z'");
    const endStr = format(end, "yyyyMMdd'T'235959'Z'");

    const body = `<?xml version="1.0" encoding="utf-8" ?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
    <c:calendar-data/>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT">
        <c:time-range start="${startStr}" end="${endStr}"/>
      </c:comp-filter>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>`;

    const response = await fetch(calendarUrl, {
      method: 'REPORT',
      headers: {
        Authorization: this.auth.authHeader(),
        'Content-Type': 'application/xml; charset=utf-8',
        Depth: '1',
      },
      body,
    });

    if (!response.ok) {
      const text = await response.text();
      throw new Error(`REPORT failed: ${text}`);
    }

    const xml = await response.text();
    return this.parseCalendarMultiget(xml, calendarUrl);
  }

  /**
   * Delete an event by its UID
   */
  async deleteEvent(
    calendarUrl: string,
    eventUid: string,
    etag: string | null
  ): Promise<void> {
    const eventUrl = `${calendarUrl.replace(/\/$/, '')}/${eventUid}.ics`;

    const headers: Record<string, string> = {
      Authorization: this.auth.authHeader(),
    };

    if (etag) {
      headers['If-Match'] = `"${etag}"`;
    }

    const response = await fetch(eventUrl, {
      method: 'DELETE',
      headers,
    });

    if (!response.ok) {
      const text = await response.text();
      throw new Error(`Failed to delete event: ${text}`);
    }
  }

  private async discoverPrincipal(): Promise<string> {
    const body = `<?xml version="1.0" encoding="utf-8" ?>
<d:propfind xmlns:d="DAV:">
  <d:prop>
    <d:current-user-principal/>
  </d:prop>
</d:propfind>`;

    const response = await fetch(CALDAV_SERVER, {
      method: 'PROPFIND',
      headers: {
        Authorization: this.auth.authHeader(),
        'Content-Type': 'application/xml; charset=utf-8',
        Depth: '0',
      },
      body,
    });

    if (!response.ok) {
      const text = await response.text();
      throw new Error(`Principal discovery failed: ${text}`);
    }

    const xml = await response.text();
    const href = this.extractHref(xml, 'current-user-principal');
    if (!href) {
      throw new Error('Could not find principal URL');
    }
    return href;
  }

  private async getCalendarHome(principalUrl: string): Promise<string> {
    const url = this.resolveUrl(principalUrl);

    const body = `<?xml version="1.0" encoding="utf-8" ?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <c:calendar-home-set/>
  </d:prop>
</d:propfind>`;

    const response = await fetch(url, {
      method: 'PROPFIND',
      headers: {
        Authorization: this.auth.authHeader(),
        'Content-Type': 'application/xml; charset=utf-8',
        Depth: '0',
      },
      body,
    });

    if (!response.ok) {
      const text = await response.text();
      throw new Error(`Calendar home discovery failed: ${text}`);
    }

    const xml = await response.text();
    const href = this.extractHref(xml, 'calendar-home-set');
    if (!href) {
      throw new Error('Could not find calendar home');
    }
    return href;
  }

  private async listCalendars(calendarHome: string): Promise<CalendarInfo[]> {
    const url = this.resolveUrl(calendarHome);

    const body = `<?xml version="1.0" encoding="utf-8" ?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/">
  <d:prop>
    <d:displayname/>
    <d:resourcetype/>
    <cs:getctag/>
  </d:prop>
</d:propfind>`;

    const response = await fetch(url, {
      method: 'PROPFIND',
      headers: {
        Authorization: this.auth.authHeader(),
        'Content-Type': 'application/xml; charset=utf-8',
        Depth: '1',
      },
      body,
    });

    if (!response.ok) {
      const text = await response.text();
      throw new Error(`Calendar list failed: ${text}`);
    }

    const xml = await response.text();
    return this.parseCalendarList(xml);
  }

  private parseCalendarList(xml: string): CalendarInfo[] {
    const calendars: CalendarInfo[] = [];

    // Simple regex-based parsing for calendar responses
    const responseRegex = /<d:response[^>]*>([\s\S]*?)<\/d:response>/gi;
    let match;

    while ((match = responseRegex.exec(xml)) !== null) {
      const responseXml = match[1];
      if (!responseXml) continue;

      // Check if it's a calendar (has <d:calendar/> or <cal:calendar/>)
      const isCalendar =
        /<(?:d:|cal:)?calendar\s*\/>/i.test(responseXml) ||
        /<(?:d:|cal:)?calendar>/i.test(responseXml);

      if (!isCalendar) continue;

      // Extract href
      const hrefMatch = /<d:href>([^<]+)<\/d:href>/i.exec(responseXml);
      if (!hrefMatch?.[1]) continue;

      // Extract displayname
      const nameMatch = /<d:displayname>([^<]*)<\/d:displayname>/i.exec(responseXml);

      calendars.push({
        url: this.resolveUrl(hrefMatch[1]),
        name: nameMatch?.[1] ?? null,
      });
    }

    return calendars;
  }

  private parseCalendarMultiget(xml: string, calendarUrl: string): ICalEvent[] {
    const events: ICalEvent[] = [];

    // Extract response blocks
    const responseRegex = /<d:response[^>]*>([\s\S]*?)<\/d:response>/gi;
    let match;

    while ((match = responseRegex.exec(xml)) !== null) {
      const responseXml = match[1];
      if (!responseXml) continue;

      // Extract etag
      const etagMatch = /<d:getetag>"?([^"<]+)"?<\/d:getetag>/i.exec(responseXml);
      const etag = etagMatch?.[1] ?? null;

      // Extract calendar-data (iCal content)
      const dataMatch = /<c:calendar-data[^>]*>([\s\S]*?)<\/c:calendar-data>/i.exec(responseXml);
      if (!dataMatch?.[1]) continue;

      // Decode HTML entities
      const icalData = dataMatch[1]
        .replace(/&lt;/g, '<')
        .replace(/&gt;/g, '>')
        .replace(/&amp;/g, '&')
        .replace(/&quot;/g, '"');

      const parsed = parseICalWithSource(icalData, calendarUrl, etag);
      events.push(...parsed);
    }

    return events;
  }

  private extractHref(xml: string, parentTag: string): string | null {
    // Simple regex to find href within parent tag
    const pattern = new RegExp(
      `<(?:d:|D:)?${parentTag}[^>]*>[\\s\\S]*?<(?:d:|D:)?href>([^<]+)<\\/(?:d:|D:)?href>`,
      'i'
    );
    const match = pattern.exec(xml);
    return match?.[1] ?? null;
  }

  private resolveUrl(path: string): string {
    if (path.startsWith('http')) {
      return path;
    }
    return `${CALDAV_SERVER}${path}`;
  }
}

/**
 * Parse an iCal VCALENDAR string into events
 */
export function parseICalWithSource(
  icalData: string,
  calendarUrl: string,
  etag: string | null
): ICalEvent[] {
  const events: ICalEvent[] = [];
  const lines = unfoldICalLines(icalData);

  let current: Partial<ICalEvent> | null = null;
  let selfPartstat: string | null = null;

  for (const line of lines) {
    const trimmed = line.trim();

    if (trimmed === 'BEGIN:VEVENT') {
      current = {
        calendarUrl,
        etag,
        attendees: [],
      };
      selfPartstat = null;
    } else if (trimmed === 'END:VEVENT' && current) {
      // Default to accepted if no PARTSTAT
      current.accepted =
        !selfPartstat ||
        selfPartstat === 'ACCEPTED';

      if (current.uid && current.dtstart) {
        events.push(current as ICalEvent);
      }
      current = null;
    } else if (current) {
      const colonIdx = trimmed.indexOf(':');
      if (colonIdx === -1) continue;

      const fullKey = trimmed.slice(0, colonIdx);
      const value = trimmed.slice(colonIdx + 1);
      const baseKey = fullKey.split(';')[0];

      switch (baseKey) {
        case 'UID':
          current.uid = value;
          break;
        case 'SUMMARY':
          current.summary = unescapeICal(value);
          break;
        case 'DTSTART': {
          const dt = parseICalDateTime(fullKey, value);
          if (dt) current.dtstart = dt;
          break;
        }
        case 'DTEND': {
          const dt = parseICalDateTime(fullKey, value);
          if (dt) current.dtend = dt;
          break;
        }
        case 'LOCATION':
          current.location = unescapeICal(value);
          break;
        case 'DESCRIPTION':
          current.description = unescapeICal(value);
          break;
        case 'URL':
          current.url = unescapeICal(value);
          break;
        case 'TRANSP':
          current.transp = value;
          break;
        case 'ATTENDEE': {
          // Check for X-IS-ME parameter
          if (fullKey.includes('X-IS-ME=TRUE')) {
            const partstat = extractPartstat(fullKey);
            if (partstat) selfPartstat = partstat;
          }
          const attendee = parseAttendee(fullKey, value);
          if (attendee) current.attendees?.push(attendee);
          break;
        }
        case 'ORGANIZER': {
          const attendee = parseAttendee(fullKey, value);
          if (attendee) {
            attendee.isOrganizer = true;
            attendee.partstat = 'ACCEPTED';
            current.attendees?.push(attendee);
          }
          break;
        }
      }
    }
  }

  return events;
}

function unfoldICalLines(data: string): string[] {
  // RFC 5545: lines are folded at 75 chars with CRLF + space/tab
  return data.replace(/\r\n[ \t]/g, '').replace(/\r?\n[ \t]/g, '').split(/\r?\n/);
}

function unescapeICal(value: string): string {
  return value
    .replace(/\\n/g, '\n')
    .replace(/\\,/g, ',')
    .replace(/\\;/g, ';')
    .replace(/\\\\/g, '\\');
}

function parseICalDateTime(key: string, value: string): EventTime | null {
  // Check for VALUE=DATE (all-day event)
  if (key.includes('VALUE=DATE') && !key.includes('VALUE=DATE-TIME')) {
    // Format: YYYYMMDD
    if (value.length >= 8) {
      const year = value.slice(0, 4);
      const month = value.slice(4, 6);
      const day = value.slice(6, 8);
      return { type: 'date', date: `${year}-${month}-${day}` };
    }
    return null;
  }

  // Parse datetime: YYYYMMDDTHHMMSS or YYYYMMDDTHHMMSSZ
  if (value.length >= 15) {
    const year = parseInt(value.slice(0, 4), 10);
    const month = parseInt(value.slice(4, 6), 10) - 1;
    const day = parseInt(value.slice(6, 8), 10);
    const hour = parseInt(value.slice(9, 11), 10);
    const minute = parseInt(value.slice(11, 13), 10);
    const second = parseInt(value.slice(13, 15), 10);

    // Check for UTC indicator
    const isUtc = value.endsWith('Z');

    if (isUtc) {
      return { type: 'dateTime', dateTime: new Date(Date.UTC(year, month, day, hour, minute, second)) };
    } else {
      // Assume local time
      return { type: 'dateTime', dateTime: new Date(year, month, day, hour, minute, second) };
    }
  }

  // Try parsing as date only
  if (value.length >= 8) {
    const year = value.slice(0, 4);
    const month = value.slice(4, 6);
    const day = value.slice(6, 8);
    return { type: 'date', date: `${year}-${month}-${day}` };
  }

  return null;
}

function extractPartstat(key: string): string | null {
  const match = /PARTSTAT=([^;:]+)/i.exec(key);
  return match?.[1] ?? null;
}

function parseAttendee(key: string, value: string): ICalAttendee | null {
  // Extract email from mailto: URI
  const email = value.replace(/^mailto:/i, '').trim();
  if (!email || !email.includes('@')) return null;

  // Extract CN (common name)
  const cnMatch = /CN=([^;:]+)/i.exec(key);
  const name = cnMatch?.[1]?.replace(/"/g, '') ?? null;

  // Extract PARTSTAT
  const partstat = extractPartstat(key) ?? 'NEEDS-ACTION';

  return {
    name,
    email,
    partstat,
    isOrganizer: false,
  };
}
