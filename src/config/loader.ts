import { readFileSync, existsSync } from 'fs';
import { getConfigPath } from './paths.js';

/**
 * Google Calendar configuration
 */
export interface GoogleConfig {
  clientId: string;
  clientSecret: string;
  calendarId: string;
}

/**
 * iCloud Calendar configuration
 */
export interface ICloudConfig {
  appleId: string;
  appPassword: string;
}

/**
 * Root configuration structure
 */
export interface Config {
  google: GoogleConfig | null;
  icloud: ICloudConfig | null;
}

/**
 * Load configuration from disk
 */
export function loadConfig(): Config {
  const path = getConfigPath();

  if (!existsSync(path)) {
    return { google: null, icloud: null };
  }

  try {
    const content = readFileSync(path, 'utf-8');
    const raw = JSON.parse(content) as Record<string, unknown>;

    const config: Config = { google: null, icloud: null };

    // Parse Google config
    if (raw.google && typeof raw.google === 'object') {
      const g = raw.google as Record<string, unknown>;
      if (typeof g.client_id === 'string' && typeof g.client_secret === 'string') {
        config.google = {
          clientId: g.client_id,
          clientSecret: g.client_secret,
          calendarId: typeof g.calendar_id === 'string' ? g.calendar_id : 'primary',
        };
      }
    }

    // Parse iCloud config
    if (raw.icloud && typeof raw.icloud === 'object') {
      const i = raw.icloud as Record<string, unknown>;
      if (typeof i.apple_id === 'string' && typeof i.app_password === 'string') {
        config.icloud = {
          appleId: i.apple_id,
          appPassword: i.app_password,
        };
      }
    }

    return config;
  } catch {
    return { google: null, icloud: null };
  }
}
