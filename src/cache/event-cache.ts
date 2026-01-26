import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'fs';
import { dirname } from 'path';
import { getEventsCachePath, getCacheDir } from '../config/paths.js';
import type { DisplayEvent } from '../types/events.js';

/**
 * Serializable cache format for disk persistence
 */
interface DiskCache {
  google: Record<string, DisplayEvent[]>;
  icloud: Record<string, DisplayEvent[]>;
}

/**
 * Source-specific event cache
 */
export class SourceCache {
  private byDate: Map<string, DisplayEvent[]> = new Map();
  private fetchedMonths: Set<string> = new Set();

  /**
   * Check if a month has been fetched
   */
  hasMonth(date: Date): boolean {
    const key = `${date.getFullYear()}-${date.getMonth() + 1}`;
    return this.fetchedMonths.has(key);
  }

  /**
   * Store events for a month, replacing any existing events
   */
  store(events: DisplayEvent[], monthDate: Date): void {
    const year = monthDate.getFullYear();
    const month = monthDate.getMonth() + 1;
    const monthKey = `${year}-${month}`;

    // Clear existing events for this month
    for (const [dateKey] of this.byDate) {
      const [y, m] = dateKey.split('-').map(Number);
      if (y === year && m === month) {
        this.byDate.delete(dateKey);
      }
    }

    // Store new events grouped by date
    for (const event of events) {
      const existing = this.byDate.get(event.date) ?? [];
      existing.push(event);
      this.byDate.set(event.date, existing);
    }

    this.fetchedMonths.add(monthKey);
  }

  /**
   * Get events for a specific date
   */
  get(date: string): DisplayEvent[] {
    return this.byDate.get(date) ?? [];
  }

  /**
   * Check if there are events on a specific date
   */
  hasEvents(date: string): boolean {
    const events = this.byDate.get(date);
    return events !== undefined && events.length > 0;
  }

  /**
   * Clear all cached data
   */
  clear(): void {
    this.byDate.clear();
    this.fetchedMonths.clear();
  }

  /**
   * Get raw data for serialization
   */
  rawData(): Record<string, DisplayEvent[]> {
    const result: Record<string, DisplayEvent[]> = {};
    for (const [key, value] of this.byDate) {
      result[key] = value;
    }
    return result;
  }

  /**
   * Load from raw data (for cache restore)
   * Note: Does not mark months as fetched to force refresh
   */
  loadFrom(data: Record<string, DisplayEvent[]>): void {
    this.byDate.clear();
    for (const [key, value] of Object.entries(data)) {
      this.byDate.set(key, value);
    }
    // Don't mark months as fetched - we want to refresh from network
  }
}

/**
 * Combined event cache for all sources
 */
export class EventCache {
  public google = new SourceCache();
  public icloud = new SourceCache();

  /**
   * Check if any source has events on this date
   */
  hasEvents(date: string): boolean {
    return this.google.hasEvents(date) || this.icloud.hasEvents(date);
  }

  /**
   * Clear all caches
   */
  clear(): void {
    this.google.clear();
    this.icloud.clear();
  }

  /**
   * Save cache to disk
   */
  saveToDisk(): void {
    try {
      const dir = getCacheDir();
      if (!existsSync(dir)) {
        mkdirSync(dir, { recursive: true });
      }

      const cache: DiskCache = {
        google: this.google.rawData(),
        icloud: this.icloud.rawData(),
      };

      writeFileSync(getEventsCachePath(), JSON.stringify(cache));
    } catch {
      // Ignore save errors
    }
  }

  /**
   * Load cache from disk
   */
  loadFromDisk(): boolean {
    const path = getEventsCachePath();
    if (!existsSync(path)) return false;

    try {
      const content = readFileSync(path, 'utf-8');
      const cache = JSON.parse(content) as DiskCache;

      if (cache.google) {
        this.google.loadFrom(cache.google);
      }
      if (cache.icloud) {
        this.icloud.loadFrom(cache.icloud);
      }

      return true;
    } catch {
      return false;
    }
  }
}
