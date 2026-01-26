import React from 'react';
import { Box, Text } from 'ink';
import type { DisplayEvent } from '../types/events.js';
import { sortAttendees, getStatusIcon } from '../types/events.js';

interface EventDetailsProps {
  event: DisplayEvent | null;
}

export function EventDetails({ event }: EventDetailsProps): JSX.Element {
  if (!event) {
    return (
      <Box flexDirection="column">
        <Text color="gray">No event selected</Text>
        <Text color="gray" dimColor>
          Press Enter to select an event
        </Text>
      </Box>
    );
  }

  const sortedAttendees = sortAttendees(event.attendees);

  return (
    <Box flexDirection="column" paddingLeft={1}>
      {/* Title */}
      <Text bold color="white">
        {event.title}
      </Text>

      {/* Time */}
      <Box marginTop={1}>
        <Text color="gray">Time: </Text>
        <Text color="white">
          {event.timeStr}
          {event.endTimeStr && event.timeStr !== 'All day'
            ? ` - ${event.endTimeStr}`
            : ''}
        </Text>
      </Box>

      {/* Location */}
      {event.location && (
        <Box>
          <Text color="gray">Location: </Text>
          <Text color="yellow">{truncate(event.location, 40)}</Text>
        </Box>
      )}

      {/* Meeting URL */}
      {event.meetingUrl && (
        <Box>
          <Text color="gray">Meeting: </Text>
          <Text color="green">[J]oin</Text>
        </Box>
      )}

      {/* Calendar */}
      <Box>
        <Text color="gray">Calendar: </Text>
        <Text color={event.id.type === 'google' ? 'blue' : 'magenta'}>
          {event.id.type === 'google'
            ? event.id.calendarName ?? 'Google Calendar'
            : event.id.calendarName ?? 'iCloud Calendar'}
        </Text>
      </Box>

      {/* Attendees */}
      {sortedAttendees.length > 0 && (
        <Box flexDirection="column" marginTop={1}>
          <Text color="gray">Attendees ({sortedAttendees.length}):</Text>
          {sortedAttendees.slice(0, 8).map((att, idx) => {
            let color: string;
            switch (att.status) {
              case 'organizer':
                color = 'blue';
                break;
              case 'accepted':
                color = 'green';
                break;
              case 'declined':
                color = 'red';
                break;
              case 'tentative':
                color = 'yellow';
                break;
              default:
                color = 'gray';
            }

            return (
              <Box key={idx} paddingLeft={1}>
                <Text color={color}>{getStatusIcon(att.status)} </Text>
                <Text color="white">{att.name ?? att.email}</Text>
              </Box>
            );
          })}
          {sortedAttendees.length > 8 && (
            <Box paddingLeft={1}>
              <Text color="gray">
                ... and {sortedAttendees.length - 8} more
              </Text>
            </Box>
          )}
        </Box>
      )}

      {/* Description */}
      {event.description && (
        <Box flexDirection="column" marginTop={1}>
          <Text color="gray">Description:</Text>
          <Text color="white">{truncate(event.description, 200)}</Text>
        </Box>
      )}

      {/* Actions hint */}
      <Box marginTop={1}>
        <Text color="gray" dimColor>
          {event.id.type === 'google' && !event.isOrganizer
            ? '[a]ccept [d]ecline '
            : ''}
          [x]delete {event.meetingUrl ? '[J]oin ' : ''}[Esc]back
        </Text>
      </Box>
    </Box>
  );
}

function truncate(str: string, maxLen: number): string {
  if (str.length <= maxLen) return str;
  return str.slice(0, maxLen - 1) + '…';
}
