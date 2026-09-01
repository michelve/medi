/**
 * Root component.
 *
 * Startup order matters: `configureTVRemoteControl()` must run BEFORE any
 * spatial-navigation component mounts (react-tv-space-navigation requirement), so
 * we call it at module load, above the component.
 *
 * The app renders a small route stack (`Home → Detail → Player`). Every screen in
 * the stack is mounted, but only the top one gets `isActive`, so react-tv-space-
 * navigation gives D-pad focus solely to the visible screen — background screens'
 * nodes are inert. The hardware menu/back button pops the stack.
 */

import React, { useEffect } from 'react';
import { StyleSheet, View, useTVEventHandler } from 'react-native';
import { StatusBar } from 'expo-status-bar';

import { configureTVRemoteControl } from './deps';
import { ApiProvider } from './api';
import { NavigationProvider, useNavigation, type Route } from './navigation';
import { HomeScreen } from './screens/HomeScreen';
import { DetailScreen } from './screens/DetailScreen';
import { PlayerScreen } from './screens/PlayerScreen';

// Register the native remote → spatial-navigation bridge exactly once, before
// any focusable mounts.
configureTVRemoteControl();

export default function App(): React.JSX.Element {
  return (
    <ApiProvider>
      <NavigationProvider initial={{ name: 'Home' }}>
        <View style={styles.root}>
          <StatusBar hidden />
          <RouteStack />
        </View>
      </NavigationProvider>
    </ApiProvider>
  );
}

/**
 * Render the whole stack; each screen receives `isActive = is top of stack`. We
 * keep lower screens mounted (so back is instant and their scroll position is
 * preserved) but inactive for focus.
 */
function RouteStack(): React.JSX.Element {
  const nav = useNavigation();

  // Hardware back / menu button pops the stack.
  useTVEventHandler((evt: { eventType: string }) => {
    if (evt.eventType === 'menu' && nav.canGoBack) {
      nav.pop();
    }
  });

  const topIndex = nav.stack.length - 1;

  return (
    <>
      {nav.stack.map((route, index) => (
        <ScreenHost key={routeKey(route, index)} route={route} isActive={index === topIndex}>
          {renderRoute(route, index === topIndex)}
        </ScreenHost>
      ))}
    </>
  );
}

/** Absolutely-fill host so stacked screens overlay each other (top one visible). */
function ScreenHost({
  route,
  isActive,
  children,
}: {
  route: Route;
  isActive: boolean;
  children: React.ReactNode;
}): React.JSX.Element {
  return (
    <View
      style={[StyleSheet.absoluteFill, !isActive && styles.hidden]}
      // Background screens don't receive touches/pointer either.
      pointerEvents={isActive ? 'auto' : 'none'}
    >
      {children}
    </View>
  );
}

function renderRoute(route: Route, isActive: boolean): React.JSX.Element {
  switch (route.name) {
    case 'Home':
      return <HomeScreen isActive={isActive} />;
    case 'Detail':
      return <DetailScreen kind={route.kind} id={route.id} isActive={isActive} />;
    case 'Player':
      return <PlayerScreen fileId={route.fileId} title={route.title} isActive={isActive} />;
  }
}

function routeKey(route: Route, index: number): string {
  switch (route.name) {
    case 'Home':
      return `home-${index}`;
    case 'Detail':
      return `detail-${route.kind}-${route.id}-${index}`;
    case 'Player':
      return `player-${route.fileId}-${index}`;
  }
}

const styles = StyleSheet.create({
  root: {
    flex: 1,
    backgroundColor: '#0b0b0f',
  },
  hidden: {
    // Keep mounted but visually behind + non-interactive.
    opacity: 0,
  },
});
