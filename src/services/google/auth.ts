import type { GoogleConfig } from '../../config/loader.js';
import type { TokenInfo } from '../../types/auth.js';
import type { DeviceCodeResponse, TokenResponse } from './types.js';

const DEVICE_CODE_URL = 'https://oauth2.googleapis.com/device/code';
const TOKEN_URL = 'https://oauth2.googleapis.com/token';
const CALENDAR_SCOPE = 'https://www.googleapis.com/auth/calendar';

export type PollResult =
  | { type: 'success'; tokens: TokenInfo }
  | { type: 'pending' }
  | { type: 'slowDown' }
  | { type: 'denied' }
  | { type: 'expired' };

/**
 * Google OAuth device flow authentication
 */
export class GoogleAuth {
  constructor(private config: GoogleConfig) {}

  /**
   * Step 1: Request device code
   */
  async requestDeviceCode(): Promise<DeviceCodeResponse> {
    const response = await fetch(DEVICE_CODE_URL, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/x-www-form-urlencoded',
      },
      body: new URLSearchParams({
        client_id: this.config.clientId,
        scope: CALENDAR_SCOPE,
      }),
    });

    if (!response.ok) {
      const body = await response.text();
      throw new Error(`Failed to get device code: ${body}`);
    }

    return (await response.json()) as DeviceCodeResponse;
  }

  /**
   * Step 2: Poll for token (call this repeatedly)
   */
  async pollForToken(deviceCode: string): Promise<PollResult> {
    const response = await fetch(TOKEN_URL, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/x-www-form-urlencoded',
      },
      body: new URLSearchParams({
        client_id: this.config.clientId,
        client_secret: this.config.clientSecret,
        device_code: deviceCode,
        grant_type: 'urn:ietf:params:oauth:grant-type:device_code',
      }),
    });

    if (response.ok) {
      const tokenResponse = (await response.json()) as TokenResponse;
      const expiresAt = new Date(Date.now() + tokenResponse.expires_in * 1000);

      return {
        type: 'success',
        tokens: {
          accessToken: tokenResponse.access_token,
          refreshToken: tokenResponse.refresh_token ?? null,
          expiresAt: expiresAt.toISOString(),
          tokenType: tokenResponse.token_type,
        },
      };
    }

    const error = (await response.json()) as { error?: string };
    switch (error.error) {
      case 'authorization_pending':
        return { type: 'pending' };
      case 'slow_down':
        return { type: 'slowDown' };
      case 'access_denied':
        return { type: 'denied' };
      case 'expired_token':
        return { type: 'expired' };
      default:
        throw new Error(`Unknown error: ${JSON.stringify(error)}`);
    }
  }

  /**
   * Refresh an expired token
   */
  async refreshToken(refreshToken: string): Promise<TokenInfo> {
    const response = await fetch(TOKEN_URL, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/x-www-form-urlencoded',
      },
      body: new URLSearchParams({
        client_id: this.config.clientId,
        client_secret: this.config.clientSecret,
        refresh_token: refreshToken,
        grant_type: 'refresh_token',
      }),
    });

    if (!response.ok) {
      const body = await response.text();
      throw new Error(`Failed to refresh token: ${body}`);
    }

    const tokenResponse = (await response.json()) as TokenResponse;
    const expiresAt = new Date(Date.now() + tokenResponse.expires_in * 1000);

    return {
      accessToken: tokenResponse.access_token,
      refreshToken: refreshToken, // Keep original
      expiresAt: expiresAt.toISOString(),
      tokenType: tokenResponse.token_type,
    };
  }
}
