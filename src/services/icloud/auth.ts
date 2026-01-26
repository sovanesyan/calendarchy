import type { ICloudConfig } from '../../config/loader.js';

/**
 * iCloud Basic auth helper
 */
export class ICloudAuth {
  constructor(private config: ICloudConfig) {}

  /**
   * Get the Authorization header value
   */
  authHeader(): string {
    const credentials = Buffer.from(
      `${this.config.appleId}:${this.config.appPassword}`
    ).toString('base64');
    return `Basic ${credentials}`;
  }
}
