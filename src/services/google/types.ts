/**
 * Device code response from Google OAuth
 */
export interface DeviceCodeResponse {
  device_code: string;
  user_code: string;
  verification_url: string;
  expires_in: number;
}

/**
 * Token endpoint response
 */
export interface TokenResponse {
  access_token: string;
  refresh_token?: string;
  expires_in: number;
  token_type: string;
}

/**
 * Event attendee from API
 */
export interface Attendee {
  email?: string;
  displayName?: string;
  responseStatus?: string;
  self?: boolean;
  organizer?: boolean;
}

/**
 * Conference entry point
 */
export interface EntryPoint {
  entryPointType?: string;
  uri?: string;
}

/**
 * Conference data
 */
export interface ConferenceData {
  entryPoints?: EntryPoint[];
}

/**
 * Event date/time
 */
export interface EventDateTime {
  date?: string; // For all-day events (YYYY-MM-DD)
  dateTime?: string; // For timed events (ISO datetime)
  timeZone?: string;
}

/**
 * Calendar event from API
 */
export interface CalendarEvent {
  id: string;
  summary?: string;
  start: EventDateTime;
  end: EventDateTime;
  location?: string;
  description?: string;
  status?: string;
  transparency?: string; // "transparent" = free, "opaque" = busy
  attendees?: Attendee[];
  conferenceData?: ConferenceData;
  hangoutLink?: string;
}

/**
 * Events list response
 */
export interface EventsListResponse {
  items?: CalendarEvent[];
  nextPageToken?: string;
}

/**
 * Calendar metadata
 */
export interface CalendarMeta {
  summary?: string;
}
