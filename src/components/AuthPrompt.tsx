import React from 'react';
import { Box, Text } from 'ink';

interface AuthPromptProps {
  userCode: string;
  verificationUrl: string;
}

export function AuthPrompt({ userCode, verificationUrl }: AuthPromptProps): JSX.Element {
  return (
    <Box
      flexDirection="column"
      borderStyle="round"
      borderColor="cyan"
      padding={1}
    >
      <Text color="cyan" bold>
        Google Authentication
      </Text>
      <Box marginTop={1} flexDirection="column">
        <Text>1. Visit: <Text color="blue" bold>{verificationUrl}</Text></Text>
        <Text>2. Enter code: <Text color="yellow" bold>{userCode}</Text></Text>
        <Text>3. Waiting for authorization...</Text>
      </Box>
      <Box marginTop={1}>
        <Text color="gray" dimColor>Press Esc to cancel</Text>
      </Box>
    </Box>
  );
}
