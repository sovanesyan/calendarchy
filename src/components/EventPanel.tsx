import React from 'react';
import { Box, Text } from 'ink';
import { useAppState } from '../store/AppContext.js';
import { useSelectedDateEvents } from '../hooks/useNavigation.js';
import type { DisplayEvent } from '../types/events.js';
import { format, parseISO } from 'date-fns';

interface EventPanelProps {
  maxHeight?: number;
}

export function EventPanel({ maxHeight = 20 }: EventPanelProps): JSX.Element {
  const state = useAppState();
  const events = useSelectedDateEvents();

  const selectedDate = parseISO(state.selectedDate);
  const dateStr = format(selectedDate, 'EEEE, MMMM d');

  return (
    <Box flexDirection="column" flexGrow={1}>
      {/* Date header */}
      <Box marginBottom={1}>
        <Text bold color="cyan">
          {dateStr}
        </Text>
        {state.googleLoading || state.icloudLoading ? (
          <Text color="yellow"> (loading...)</Text>
        ) : null}
      </Box>

      {/* Events list */}
      {events.length === 0 ? (
        <Text color="gray">No events</Text>
      ) : (
        <Box flexDirection="column">
          {events.slice(0, maxHeight).map((event, idx) => {
            const eventKey = event.id.type === 'google'
              ? `google-${event.id.eventId ?? idx}`
              : `icloud-${event.id.eventUid ?? idx}`;
            return (
              <EventRow
                key={eventKey}
                event={event}
                isSelected={
                  state.navigationMode === 'event' && idx === state.selectedEventIndex
                }
              />
            );
          })}
          {events.length > maxHeight && (
            <Text color="gray">... and {events.length - maxHeight} more</Text>
          )}
        </Box>
      )}
    </Box>
  );
}

interface EventRowProps {
  event: DisplayEvent;
  isSelected: boolean;
}

function EventRow({ event, isSelected }: EventRowProps): JSX.Element {
  // Color based on source
  const sourceColor = event.id.type === 'google' ? 'blue' : 'magenta';

  // Determine event state color
  let stateColor: string | undefined;
  if (event.isFree) {
    stateColor = 'gray';
  } else if (!event.accepted) {
    stateColor = 'gray';
  }

  // Check if event is current or past
  const now = new Date();
  const todayStr = format(now, 'yyyy-MM-dd');
  const isToday = event.date === todayStr;

  if (isToday && event.timeStr !== 'All day' && event.endTimeStr) {
    const [endHour, endMin] = event.endTimeStr.split(':').map(Number);
    const endTime = new Date(now);
    endTime.setHours(endHour!, endMin!, 0);

    if (now > endTime) {
      stateColor = 'gray'; // Past event
    } else {
      const [startHour, startMin] = event.timeStr.split(':').map(Number);
      const startTime = new Date(now);
      startTime.setHours(startHour!, startMin!, 0);

      if (now >= startTime && now <= endTime) {
        stateColor = 'green'; // Current event
      }
    }
  }

  // Format time string
  let timeDisplay = event.timeStr ?? '';
  if (event.endTimeStr && event.timeStr && event.timeStr !== 'All day') {
    timeDisplay = `${event.timeStr}-${event.endTimeStr}`;
  }

  // Truncate title if needed
  const maxTitleLen = 30;
  const eventTitle = event.title ?? '(No title)';
  const title =
    eventTitle.length > maxTitleLen
      ? eventTitle.slice(0, maxTitleLen - 1) + '…'
      : eventTitle;

  return (
    <Box>
      {/* Selection indicator */}
      <Text color={isSelected ? 'cyan' : undefined}>
        {isSelected ? '>' : ' '}
      </Text>

      {/* Source indicator */}
      <Text color={sourceColor}>
        {event.id.type === 'google' ? 'G' : 'i'}
      </Text>
      <Text> </Text>

      {/* Time */}
      <Text color={stateColor ?? 'white'}>
        {timeDisplay.padEnd(11)}
      </Text>
      <Text> </Text>

      {/* Title */}
      <Text color={stateColor ?? 'white'} bold={isSelected}>
        {title}
      </Text>

      {/* Meeting indicator */}
      {event.meetingUrl && (
        <Text color="yellow"> [mtg]</Text>
      )}
    </Box>
  );
}
