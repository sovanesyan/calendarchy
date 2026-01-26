import React, { createContext, useContext, useReducer, type Dispatch, type ReactNode } from 'react';
import type { AppState } from './types.js';
import { reducer, type Action } from './reducer.js';

interface AppContextValue {
  state: AppState;
  dispatch: Dispatch<Action>;
}

const AppContext = createContext<AppContextValue | null>(null);

interface AppProviderProps {
  initialState: AppState;
  children: ReactNode;
}

export function AppProvider({ initialState, children }: AppProviderProps): JSX.Element {
  const [state, dispatch] = useReducer(reducer, initialState);

  return (
    <AppContext.Provider value={{ state, dispatch }}>
      {children}
    </AppContext.Provider>
  );
}

export function useAppState(): AppState {
  const context = useContext(AppContext);
  if (!context) {
    throw new Error('useAppState must be used within AppProvider');
  }
  return context.state;
}

export function useAppDispatch(): Dispatch<Action> {
  const context = useContext(AppContext);
  if (!context) {
    throw new Error('useAppDispatch must be used within AppProvider');
  }
  return context.dispatch;
}

export function useApp(): AppContextValue {
  const context = useContext(AppContext);
  if (!context) {
    throw new Error('useApp must be used within AppProvider');
  }
  return context;
}
