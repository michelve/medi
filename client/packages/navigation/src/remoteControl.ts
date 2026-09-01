/**
 * Bridges the react-native-tvos D-pad/remote hardware events into
 * react-tv-space-navigation's deterministic focus engine.
 *
 * `react-tv-space-navigation` does not read native focus itself — it needs to be
 * told which *direction* the user pressed and then it computes the next focused
 * node deterministically (README §Spatial Navigation: we deliberately avoid the
 * flaky native nearest-neighbor heuristic). We subscribe to the raw tvOS/Android
 * TV remote via `TVEventHandler` and map each key to a `Direction`.
 *
 * Call `configureTVRemoteControl()` exactly once at app startup, BEFORE any
 * spatial-navigation component mounts (`SpatialNavigation.configureRemoteControl`
 * has that requirement).
 */

import { SpatialNavigation, Directions } from 'react-tv-space-navigation';
// `TVEventHandler` ships with react-native-tvos.
import { TVEventHandler } from 'react-native';

/** The subset of tvOS/Android TV remote key names we route to directions. */
type TVKeyAction =
  | 'up'
  | 'down'
  | 'left'
  | 'right'
  | 'select'
  | 'longSelect'
  | 'playPause'
  | 'menu'
  | (string & {});

interface TVRemoteEvent {
  eventType: TVKeyAction;
  eventKeyAction?: number; // 0 = down (key press), 1 = up (release)
}

/**
 * Map a raw remote `eventType` to a spatial-navigation `Direction`. `select`
 * becomes `ENTER`. `menu`/`playPause` are intentionally NOT mapped — the app's
 * back handler and the player overlay (`useTVEventHandler`, Phase 5) own those.
 */
function toDirection(eventType: TVKeyAction): Directions | null {
  switch (eventType) {
    case 'up':
      return Directions.UP;
    case 'down':
      return Directions.DOWN;
    case 'left':
      return Directions.LEFT;
    case 'right':
      return Directions.RIGHT;
    case 'select':
      return Directions.ENTER;
    default:
      return null;
  }
}

type Handler = (evt: TVRemoteEvent) => void;

/**
 * Register the native remote listener with the spatial navigator.
 *
 * `configureRemoteControl` is given a subscriber (returns an opaque handle) and
 * an unsubscriber (tears it down). We debounce on `eventKeyAction`: react to the
 * key-DOWN (0) only, so a single press moves focus once — a press+release must
 * not fire two moves.
 */
export function configureTVRemoteControl(): void {
  SpatialNavigation.configureRemoteControl({
    remoteControlSubscriber: (callback: (d: Directions) => void) => {
      const handler: Handler = (evt) => {
        // Only act on the key-press edge, not the release.
        if (evt.eventKeyAction != null && evt.eventKeyAction !== 0) return;
        const direction = toDirection(evt.eventType);
        if (direction !== null) callback(direction);
      };

      // react-native-tvos exposes TVEventHandler in two shapes across versions:
      // the class form (`new TVEventHandler()` + `.enable(component, cb)`) and
      // the newer functional form (`TVEventHandler.addListener(cb)` → sub).
      const anyTV = TVEventHandler as unknown as {
        addListener?: (cb: Handler) => { remove: () => void };
      };

      if (typeof anyTV.addListener === 'function') {
        return anyTV.addListener(handler);
      }

      // Legacy class API.
      const legacy = new (TVEventHandler as unknown as {
        new (): { enable: (c: unknown, cb: Handler) => void; disable: () => void };
      })();
      legacy.enable(null, handler);
      return legacy;
    },

    remoteControlUnsubscriber: (
      subscription: { remove?: () => void; disable?: () => void } | null,
    ) => {
      if (!subscription) return;
      if (typeof subscription.remove === 'function') subscription.remove();
      else if (typeof subscription.disable === 'function') subscription.disable();
    },
  });
}
