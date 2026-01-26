import { homedir } from 'os';
import { join } from 'path';

/**
 * Get the config directory path (~/.config/calendarchy)
 */
export function getConfigDir(): string {
  return join(homedir(), '.config', 'calendarchy');
}

/**
 * Get the config file path (~/.config/calendarchy/config.json)
 */
export function getConfigPath(): string {
  return join(getConfigDir(), 'config.json');
}

/**
 * Get the tokens file path (~/.config/calendarchy/tokens.json)
 */
export function getTokenPath(): string {
  return join(getConfigDir(), 'tokens.json');
}

/**
 * Get the cache directory path (~/.cache/calendarchy)
 */
export function getCacheDir(): string {
  return join(homedir(), '.cache', 'calendarchy');
}

/**
 * Get the events cache file path (~/.cache/calendarchy/events.json)
 */
export function getEventsCachePath(): string {
  return join(getCacheDir(), 'events.json');
}
