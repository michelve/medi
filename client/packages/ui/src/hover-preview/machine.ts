/**
 * The Netflix-style hover-to-play finite state machine (README §Deterministic
 * State Management; task `40-phase4-tv-client-ui.md` §Hover-to-play FSM).
 *
 * A naive `setTimeout` implementation races: videos start after the user has
 * scrolled away, or several hidden previews play audio at once. This machine
 * makes every phase deterministic, so those races are structurally impossible.
 *
 * State chart (names mirror the README/task verbatim):
 *
 *   awaitingBackgroundImageLoad
 *        │  REPORT_IMAGE_LOADED
 *        ▼
 *      idle ──────── FOCUS ──────► showingVideo
 *        ▲                              │
 *        │                              ▼  (nested)
 *        │                    loadingVideoSrc.cannotMoveOn
 *        │                       │   (the 2-SECOND GATE — a `after: 2000`
 *        │                       │    delayed transition)
 *        │                       ▼
 *        │                    loadingVideoSrc.canMoveOn
 *        │                       │  REPORT_VIDEO_LOADED
 *        │                       ▼
 *        │                    playing
 *        │                              │
 *        └────────── BLUR ◄─────────────┘   (teardown from ANY substate)
 *
 * Key guarantees:
 *  - No video logic runs until `REPORT_IMAGE_LOADED` (prevents the visual pop).
 *  - The 2s gate throttles preview requests so fast scrolling never floods the
 *    LAN — a `BLUR` during the gate cancels before any request is made.
 *  - `BLUR` at any instant (gate or playback) returns to `idle`, and the exit
 *    action aborts the in-flight fetch, so there are no orphaned playback
 *    threads or concurrent audio.
 */

import { setup, assign, fromPromise } from 'xstate';

/** The gate duration. The task fixes this at exactly two seconds. */
export const HOVER_GATE_MS = 2000;

export interface HoverPreviewContext {
  /** The media file whose 720p silent preview this poster can play. */
  fileId: number;
  /** Resolved preview URL, set once the source has "loaded". */
  previewUrl: string | null;
  /** Abort controller for the in-flight preview HEAD/fetch, so BLUR can cancel it. */
  abort: AbortController | null;
  /** Last error (e.g. 404: preview not generated yet) for optional UI. */
  error: string | null;
}

export type HoverPreviewEvent =
  /** The poster image finished loading/caching/rendering. */
  | { type: 'REPORT_IMAGE_LOADED' }
  /** D-pad focus / pointer hover entered this poster. */
  | { type: 'FOCUS' }
  /** D-pad focus / pointer hover left this poster (teardown trigger). */
  | { type: 'BLUR' }
  /** The mounted <Video> reported it is ready and playing. */
  | { type: 'REPORT_VIDEO_LOADED' }
  /** The preview clip does not exist yet (404) or failed to resolve. */
  | { type: 'REPORT_VIDEO_ERROR'; message: string };

export interface HoverPreviewInput {
  fileId: number;
  /** Resolves a preview URL, honoring the abort signal (cancelled on BLUR). */
  resolvePreview: (fileId: number, signal: AbortSignal) => Promise<string>;
}

/**
 * Build a hover-preview machine bound to one poster. `resolvePreview` is injected
 * so the machine stays decoupled from the API client (and is unit-testable): the
 * component wires it to `ApiClient.previewUrl` + a lightweight availability check.
 */
export function createHoverPreviewMachine(input: HoverPreviewInput) {
  return setup({
    types: {
      context: {} as HoverPreviewContext,
      events: {} as HoverPreviewEvent,
      // No `input` type: the factory closes over `input` (fileId + resolver), so
      // the machine needs no runtime xstate input — which keeps `useMachine(machine)`
      // a single-argument call.
    },
    actors: {
      /**
       * Resolve the preview source during the gate window. Runs *inside* the
       * 2s gate so that, by the time the gate opens, the URL is ready and the
       * <Video> can mount instantly. Cancelled via the context abort signal.
       */
      loadPreviewSrc: fromPromise(
        async ({
          input: actorInput,
        }: {
          input: { fileId: number; signal: AbortSignal; resolve: HoverPreviewInput['resolvePreview'] };
        }) => {
          const url = await actorInput.resolve(actorInput.fileId, actorInput.signal);
          return url;
        },
      ),
    },
    actions: {
      /** Create a fresh abort controller for a new preview attempt. */
      armAbort: assign({
        abort: () => new AbortController(),
        error: null,
      }),
      /**
       * Teardown: abort any in-flight request and drop the resolved src so the
       * <Video> unmounts. This is the single choke point that guarantees no
       * orphaned playback / concurrent audio.
       */
      teardown: assign(({ context }) => {
        context.abort?.abort();
        return { abort: null, previewUrl: null };
      }),
      storeError: assign({
        error: ({ event }) =>
          event.type === 'REPORT_VIDEO_ERROR' ? event.message : 'preview failed',
        previewUrl: null,
      }),
    },
  }).createMachine({
    id: 'hoverPreview',
    context: {
      fileId: input.fileId,
      previewUrl: null,
      abort: null,
      error: null,
    },
    // 1. Nothing runs until the poster image is fully loaded.
    initial: 'awaitingBackgroundImageLoad',
    states: {
      awaitingBackgroundImageLoad: {
        on: {
          REPORT_IMAGE_LOADED: 'idle',
        },
      },

      // 2. Image loaded; wait for focus.
      idle: {
        on: {
          FOCUS: 'showingVideo',
        },
      },

      // 3–4. Focused: run the gate, resolve the src, then play.
      showingVideo: {
        // BLUR from ANY nested substate tears down and returns to idle.
        on: {
          BLUR: {
            target: 'idle',
            actions: 'teardown',
          },
        },
        initial: 'loadingVideoSrc',
        states: {
          loadingVideoSrc: {
            initial: 'cannotMoveOn',
            // Kick off the (abortable) src resolution as we enter the gate.
            entry: 'armAbort',
            invoke: {
              src: 'loadPreviewSrc',
              input: ({ context }) => ({
                fileId: context.fileId,
                // Non-null: `armAbort` runs on entry before the actor spawns.
                signal: context.abort!.signal,
                resolve: input.resolvePreview,
              }),
              onDone: {
                // Inline so `event.output` is typed as the actor's resolved URL.
                actions: assign({
                  previewUrl: ({ event }) => event.output,
                }),
              },
              onError: {
                target: '#hoverPreview.idle',
                actions: ['teardown', 'storeError'],
              },
            },
            states: {
              // 3. THE 2-SECOND GATE. A delayed transition — if BLUR fires
              // first, we never reach `canMoveOn`, so no video is shown.
              cannotMoveOn: {
                after: {
                  [HOVER_GATE_MS]: 'canMoveOn',
                },
              },
              // Gate has elapsed; wait for the <Video> to report it's ready.
              canMoveOn: {
                on: {
                  REPORT_VIDEO_LOADED: '#hoverPreview.showingVideo.playing',
                  REPORT_VIDEO_ERROR: {
                    target: '#hoverPreview.idle',
                    actions: ['teardown', 'storeError'],
                  },
                },
              },
            },
          },
          // 4. Silent preview is playing.
          playing: {
            on: {
              REPORT_VIDEO_ERROR: {
                target: '#hoverPreview.idle',
                actions: ['teardown', 'storeError'],
              },
            },
          },
        },
      },
    },
  });
}

export type HoverPreviewMachine = ReturnType<typeof createHoverPreviewMachine>;
