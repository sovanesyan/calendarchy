/**
 * Google OAuth2 tokens
 */
export interface TokenInfo {
  accessToken: string;
  refreshToken: string | null;
  expiresAt: string; // ISO datetime string
  tokenType: string;
}

/**
 * Check if a token is expired (with 5-minute buffer)
 */
export function isTokenExpired(token: TokenInfo): boolean {
  const expiresAt = new Date(token.expiresAt);
  const buffer = 5 * 60 * 1000; // 5 minutes in ms
  return Date.now() >= expiresAt.getTime() - buffer;
}

/**
 * Calendar entry with URL and display name
 */
export interface CalendarEntry {
  url: string;
  name: string | null;
}

/**
 * Google authentication state
 */
export type GoogleAuthState =
  | { type: 'notConfigured' }
  | { type: 'notAuthenticated' }
  | {
      type: 'awaitingUserCode';
      userCode: string;
      verificationUrl: string;
      deviceCode: string;
      expiresAt: string;
    }
  | { type: 'authenticated'; tokens: TokenInfo }
  | { type: 'error'; message: string };

/**
 * iCloud authentication state
 */
export type ICloudAuthState =
  | { type: 'notConfigured' }
  | { type: 'notAuthenticated' }
  | { type: 'discovering' }
  | { type: 'authenticated'; calendars: CalendarEntry[] }
  | { type: 'error'; message: string };
