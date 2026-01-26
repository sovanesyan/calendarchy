import React from 'react';
import { Box, Text } from 'ink';
import { useAppState } from '../store/AppContext.js';

export function StatusBar(): JSX.Element {
  const state = useAppState();

  // Build mode indicator
  let modeText: string;
  if (state.navigationMode === 'event') {
    modeText = 'EVENT';
  } else {
    modeText = 'DAY';
  }

  // Auth status indicators
  const googleStatus =
    state.googleAuth.type === 'authenticated'
      ? 'G✓'
      : state.googleAuth.type === 'awaitingUserCode'
        ? 'G…'
        : state.googleAuth.type === 'notConfigured'
          ? ''
          : 'G✗';

  const icloudStatus =
    state.icloudAuth.type === 'authenticated'
      ? 'i✓'
      : state.icloudAuth.type === 'discovering'
        ? 'i…'
        : state.icloudAuth.type === 'notConfigured'
          ? ''
          : 'i✗';

  return (
    <Box>
      {/* Mode indicator */}
      <Text color="cyan" bold>
        [{modeText}]
      </Text>
      <Text> </Text>

      {/* Auth status */}
      {googleStatus && (
        <>
          <Text color={state.googleAuth.type === 'authenticated' ? 'green' : 'yellow'}>
            {googleStatus}
          </Text>
          <Text> </Text>
        </>
      )}
      {icloudStatus && (
        <>
          <Text color={state.icloudAuth.type === 'authenticated' ? 'green' : 'yellow'}>
            {icloudStatus}
          </Text>
          <Text> </Text>
        </>
      )}

      {/* Status message */}
      {state.statusMessage && (
        <Text color="yellow">{state.statusMessage}</Text>
      )}

      {/* Help hint (right side) */}
      <Box flexGrow={1} justifyContent="flex-end">
        <Text color="gray" dimColor>
          [?]help [q]uit
        </Text>
      </Box>
    </Box>
  );
}
