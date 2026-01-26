#!/usr/bin/env node
import React from 'react';
import { render } from 'ink';
import { App } from './App.js';
import { AppProvider } from './store/AppContext.js';
import { createInitialState } from './store/types.js';
import { loadConfig } from './config/loader.js';
import { EventCache } from './cache/event-cache.js';

// Load configuration
const config = loadConfig();

// Create initial state
const initialState = createInitialState(config);

// Load cached events for instant display
initialState.events = new EventCache();
initialState.events.loadFromDisk();

// Mark needs fetch if authenticated
if (config.google) {
  initialState.googleNeedsFetch = true;
}
if (config.icloud) {
  initialState.icloudNeedsFetch = true;
}

// Render the app
render(
  <AppProvider initialState={initialState}>
    <App />
  </AppProvider>,
  {
    exitOnCtrlC: true,
  }
);
