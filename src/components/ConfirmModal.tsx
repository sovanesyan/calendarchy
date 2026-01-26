import React from 'react';
import { Box, Text } from 'ink';
import type { PendingAction } from '../types/navigation.js';

interface ConfirmModalProps {
  action: PendingAction;
}

export function ConfirmModal({ action }: ConfirmModalProps): JSX.Element {
  let message: string;
  switch (action.type) {
    case 'accept':
      message = `Accept "${truncate(action.eventTitle, 30)}"?`;
      break;
    case 'decline':
      message = `Decline "${truncate(action.eventTitle, 30)}"?`;
      break;
    case 'delete':
      message = `Delete "${truncate(action.eventTitle, 30)}"?`;
      break;
  }

  return (
    <Box
      flexDirection="column"
      borderStyle="round"
      borderColor="yellow"
      padding={1}
    >
      <Text color="yellow" bold>
        Confirm Action
      </Text>
      <Box marginTop={1}>
        <Text>{message}</Text>
      </Box>
      <Box marginTop={1}>
        <Text color="green">[y]es</Text>
        <Text> </Text>
        <Text color="red">[n]o</Text>
      </Box>
    </Box>
  );
}

function truncate(str: string, maxLen: number): string {
  if (str.length <= maxLen) return str;
  return str.slice(0, maxLen - 1) + '…';
}
