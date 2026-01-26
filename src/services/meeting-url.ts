/**
 * Check if a URL is a meeting URL (Zoom, Meet, Teams)
 */
export function isMeetingUrl(url: string): boolean {
  return (
    url.includes('zoom.us') ||
    url.includes('meet.google.com') ||
    url.includes('teams.microsoft.com')
  );
}

/**
 * Extract a meeting URL (Zoom, Meet, Teams) from text
 */
export function extractMeetingUrl(text: string): string | null {
  // Patterns that match any subdomain
  const flexiblePatterns = ['zoom.us/j/', 'meet.google.com/', 'teams.microsoft.com/'];

  for (const pattern of flexiblePatterns) {
    const patternPos = text.indexOf(pattern);
    if (patternPos !== -1) {
      // Find the start of the URL (search backwards for https://)
      const before = text.slice(0, patternPos);
      const httpsOffset = before.lastIndexOf('https://');
      if (httpsOffset !== -1) {
        const urlPart = text.slice(httpsOffset);
        // Find end of URL (whitespace, quote, or angle bracket)
        const endMatch = urlPart.search(/[\s"<>]/);
        const end = endMatch === -1 ? urlPart.length : endMatch;
        return urlPart.slice(0, end);
      }
    }
  }

  return null;
}
