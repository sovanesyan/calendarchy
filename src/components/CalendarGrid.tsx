import React from 'react';
import { Box, Text } from 'ink';
import {
  format,
  startOfMonth,
  endOfMonth,
  startOfWeek,
  endOfWeek,
  eachDayOfInterval,
  isSameMonth,
  isSameDay,
  parseISO,
  getISOWeek,
} from 'date-fns';
import { useAppState } from '../store/AppContext.js';

interface CalendarGridProps {
  showWeekends?: boolean;
}

export function CalendarGrid({ showWeekends = true }: CalendarGridProps): JSX.Element {
  const state = useAppState();
  const selectedDate = parseISO(state.selectedDate);
  const today = parseISO(state.currentDate);

  const monthStart = startOfMonth(selectedDate);
  const monthEnd = endOfMonth(selectedDate);
  const calendarStart = startOfWeek(monthStart, { weekStartsOn: 1 }); // Monday
  const calendarEnd = endOfWeek(monthEnd, { weekStartsOn: 1 });

  const days = eachDayOfInterval({ start: calendarStart, end: calendarEnd });

  // Group days into weeks
  const weeks: Date[][] = [];
  let currentWeek: Date[] = [];
  for (const day of days) {
    currentWeek.push(day);
    if (currentWeek.length === 7) {
      weeks.push(currentWeek);
      currentWeek = [];
    }
  }

  // Day header
  const dayHeaders = showWeekends
    ? ['Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa', 'Su']
    : ['Mo', 'Tu', 'We', 'Th', 'Fr'];

  return (
    <Box flexDirection="column">
      {/* Month/Year header */}
      <Box justifyContent="center" marginBottom={1}>
        <Text bold color="cyan">
          {format(selectedDate, 'MMMM yyyy')}
        </Text>
      </Box>

      {/* Day headers with week number column */}
      <Box>
        <Text color="gray">{'   '}</Text>
        {dayHeaders.map((day, i) => (
          <Text key={i} color="gray">
            {day}{' '}
          </Text>
        ))}
      </Box>

      {/* Calendar weeks */}
      {weeks.map((week, weekIdx) => {
        const weekNum = getISOWeek(week[0]!);
        const filteredDays = showWeekends ? week : week.slice(0, 5);

        return (
          <Box key={weekIdx}>
            {/* Week number */}
            <Text color="gray">{String(weekNum).padStart(2, ' ')} </Text>

            {/* Days */}
            {filteredDays.map((day, dayIdx) => {
              const inMonth = isSameMonth(day, selectedDate);
              const isToday = isSameDay(day, today);
              const isSelected = isSameDay(day, selectedDate);
              const hasEvents = state.events.hasEvents(format(day, 'yyyy-MM-dd'));

              let color: string | undefined;
              let bgColor: string | undefined;
              let bold = false;

              if (isSelected) {
                bgColor = 'cyan';
                color = 'black';
                bold = true;
              } else if (isToday) {
                color = 'green';
                bold = true;
              } else if (!inMonth) {
                color = 'gray';
              } else if (hasEvents) {
                color = 'white';
              } else {
                color = 'gray';
              }

              const dayStr = format(day, 'd').padStart(2, ' ');

              return (
                <Text
                  key={dayIdx}
                  color={color}
                  backgroundColor={bgColor}
                  bold={bold}
                >
                  {dayStr}{' '}
                </Text>
              );
            })}
          </Box>
        );
      })}
    </Box>
  );
}
