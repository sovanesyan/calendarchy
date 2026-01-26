import { readFileSync, writeFileSync, existsSync, mkdirSync, chmodSync } from 'fs';
import { dirname } from 'path';
import { getTokenPath, getConfigDir } from './paths.js';
import type { TokenInfo, CalendarEntry } from '../types/auth.js';

/**
 * Stored Google tokens
 */
interface GoogleTokens {
  tokens: {
    access_token: string;
    refresh_token: string | null;
    expires_at: string;
    token_type: string;
  };
  stored_at: string;
}

/**
 * Stored calendar entry (serialized format)
 */
interface StoredCalendar {
  url: string;
  name: string | null;
}

/**
 * Stored iCloud discovery info
 */
interface ICloudTokens {
  calendar_urls: string[]; // Legacy field
  calendars: StoredCalendar[];
  stored_at: string;
}

/**
 * Combined stored tokens
 */
interface StoredTokens {
  google: GoogleTokens | null;
  icloud: ICloudTokens | null;
}

function ensureConfigDir(): void {
  const dir = getConfigDir();
  if (!existsSync(dir)) {
    mkdirSync(dir, { recursive: true });
  }
}

function loadAllTokens(): StoredTokens {
  const path = getTokenPath();
  if (!existsSync(path)) {
    return { google: null, icloud: null };
  }

  try {
    const content = readFileSync(path, 'utf-8');
    return JSON.parse(content) as StoredTokens;
  } catch {
    return { google: null, icloud: null };
  }
}

function saveAllTokens(tokens: StoredTokens): void {
  ensureConfigDir();
  const path = getTokenPath();
  writeFileSync(path, JSON.stringify(tokens, null, 2));
  try {
    chmodSync(path, 0o600);
  } catch {
    // Ignore chmod errors on non-Unix systems
  }
}

/**
 * Save Google tokens to disk
 */
export function saveGoogleTokens(tokens: TokenInfo): void {
  const stored = loadAllTokens();
  stored.google = {
    tokens: {
      access_token: tokens.accessToken,
      refresh_token: tokens.refreshToken,
      expires_at: tokens.expiresAt,
      token_type: tokens.tokenType,
    },
    stored_at: new Date().toISOString(),
  };
  saveAllTokens(stored);
}

/**
 * Load Google tokens from disk
 */
export function loadGoogleTokens(): TokenInfo | null {
  const stored = loadAllTokens();
  if (!stored.google) return null;

  return {
    accessToken: stored.google.tokens.access_token,
    refreshToken: stored.google.tokens.refresh_token,
    expiresAt: stored.google.tokens.expires_at,
    tokenType: stored.google.tokens.token_type,
  };
}

/**
 * Save iCloud discovery info to disk
 */
export function saveICloudTokens(calendars: CalendarEntry[]): void {
  const stored = loadAllTokens();
  stored.icloud = {
    calendar_urls: [], // Legacy field, keep empty
    calendars: calendars.map((c) => ({ url: c.url, name: c.name })),
    stored_at: new Date().toISOString(),
  };
  saveAllTokens(stored);
}

/**
 * Load iCloud discovery info from disk
 */
export function loadICloudTokens(): CalendarEntry[] | null {
  const stored = loadAllTokens();
  if (!stored.icloud) return null;

  // Use new calendars field if available, fall back to legacy calendar_urls
  if (stored.icloud.calendars && stored.icloud.calendars.length > 0) {
    return stored.icloud.calendars.map((c) => ({
      url: c.url,
      name: c.name,
    }));
  }

  if (stored.icloud.calendar_urls && stored.icloud.calendar_urls.length > 0) {
    return stored.icloud.calendar_urls.map((url) => ({
      url,
      name: null,
    }));
  }

  return null;
}
