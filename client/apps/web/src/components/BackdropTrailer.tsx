/**
 * `BackdropTrailer` — the Apple-TV-style hero flourish (movie detail page).
 *
 * Plays a movie's official YouTube trailer *behind* the hero content: the still backdrop
 * shows first, then after a short delay the trailer fades in and plays muted (browser policy),
 * skipping the intro (`start=4`), with YouTube's chrome stripped as far as its ToS allow. On
 * end — or if the video can't be embedded — it fades out and the parent's still backdrop is
 * all that remains. A single mute/unmute chip is the only visible affordance.
 *
 * Trailers here are stored as YouTube keys only (`Trailer.youtube_key`), so this wraps the
 * YouTube IFrame Player API rather than an HTML5 <video>. The API script is loaded lazily,
 * once per page, via a module-level singleton.
 */

import { useEffect, useRef, useState } from 'react';

/** Minimal shape of the bits of the YT IFrame API we use. */
interface YTPlayer {
  mute(): void;
  unMute(): void;
  playVideo(): void;
  destroy(): void;
}
interface YTNamespace {
  Player: new (
    el: HTMLElement,
    opts: Record<string, unknown>,
  ) => YTPlayer;
  PlayerState: { PLAYING: number; ENDED: number };
}
declare global {
  interface Window {
    YT?: YTNamespace;
    onYouTubeIframeAPIReady?: () => void;
  }
}

/**
 * Load the IFrame API script once and resolve when `window.YT` is ready. Subsequent callers
 * share the same promise. Chains any pre-existing `onYouTubeIframeAPIReady` so we don't clobber
 * another consumer.
 */
let apiPromise: Promise<YTNamespace> | null = null;
function loadYouTubeApi(): Promise<YTNamespace> {
  if (apiPromise) return apiPromise;
  apiPromise = new Promise<YTNamespace>((resolve) => {
    if (window.YT?.Player) {
      resolve(window.YT);
      return;
    }
    const prev = window.onYouTubeIframeAPIReady;
    window.onYouTubeIframeAPIReady = () => {
      prev?.();
      if (window.YT) resolve(window.YT);
    };
    // Only inject the <script> once even if two players mount before it loads.
    if (!document.querySelector('script[data-yt-iframe-api]')) {
      const s = document.createElement('script');
      s.src = 'https://www.youtube.com/iframe_api';
      s.async = true;
      s.dataset.ytIframeApi = 'true';
      document.head.appendChild(s);
    }
  });
  return apiPromise;
}

export interface BackdropTrailerProps {
  /** YouTube video key to play behind the hero. */
  youtubeKey: string;
  /** Seconds to skip at the start (the intro). Default 4. */
  startSeconds?: number;
  /** Delay before the trailer fades in, so the backdrop reads first. Default 1500ms. */
  delayMs?: number;
  /** Called when the video can't be played (embedding disabled, load error), so the parent
   *  can stay on the still backdrop. */
  onUnavailable?: () => void;
}

export function BackdropTrailer({
  youtubeKey,
  startSeconds = 4,
  delayMs = 1500,
  onUnavailable,
}: BackdropTrailerProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const playerRef = useRef<YTPlayer | null>(null);
  // Visible once the video actually starts playing; drives the opacity crossfade. Reset to
  // false on end so the layer fades back out to the backdrop.
  const [playing, setPlaying] = useState(false);
  const [muted, setMuted] = useState(true);
  // Once the trailer has finished (or errored) we don't rebuild it for this key.
  const [done, setDone] = useState(false);

  // (Re)build the player whenever the key changes. Everything is torn down on cleanup so
  // navigating between movies never leaks an iframe or leaves audio playing.
  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    setPlaying(false);
    setMuted(true);
    setDone(false);

    timer = setTimeout(() => {
      loadYouTubeApi().then((YT) => {
        if (cancelled || !hostRef.current) return;
        playerRef.current = new YT.Player(hostRef.current, {
          videoId: youtubeKey,
          host: 'https://www.youtube-nocookie.com',
          playerVars: {
            autoplay: 1,
            mute: 1,
            controls: 0,
            modestbranding: 1,
            rel: 0,
            disablekb: 1,
            iv_load_policy: 3,
            cc_load_policy: 0,
            playsinline: 1,
            fs: 0,
            start: startSeconds,
          },
          events: {
            onReady: (e: { target: YTPlayer }) => {
              e.target.mute();
              e.target.playVideo();
            },
            onStateChange: (e: { data: number }) => {
              if (e.data === YT.PlayerState.PLAYING) setPlaying(true);
              else if (e.data === YT.PlayerState.ENDED) {
                setPlaying(false);
                setDone(true);
              }
            },
            onError: () => {
              setPlaying(false);
              setDone(true);
              onUnavailable?.();
            },
          },
        });
      });
    }, delayMs);

    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
      // `destroy()` removes the iframe and stops audio.
      try {
        playerRef.current?.destroy();
      } catch {
        /* player may never have been created */
      }
      playerRef.current = null;
    };
  }, [youtubeKey, startSeconds, delayMs, onUnavailable]);

  const toggleMute = () => {
    const p = playerRef.current;
    if (!p) return;
    if (muted) {
      p.unMute();
      setMuted(false);
    } else {
      p.mute();
      setMuted(true);
    }
  };

  return (
    <>
      {/* Full-bleed video layer between the backdrop <img> and the hero scrim. The YT iframe
          ignores object-fit, so we oversize it to cover a 16:9 box in any aspect (the classic
          177.78vh / 56.25vw cover transform) and then apply an OVERSCAN zoom: many trailers are
          16:9 files with baked-in black letterbox bars top & bottom (cinematic 2.39:1 content in
          a 16:9 frame ≈ 13% black each side). Scaling ~1.3× pushes those bars off the top and
          bottom edges so the hero shows picture, not black. The extra height is biased UPWARD
          (transform-origin at top) so more of the crop comes off the bottom, keeping the
          top-center focus of the frame visible. pointer-events:none lets hero actions stay
          clickable through it. Fades in only once the video is actually playing. */}
      <div
        aria-hidden="true"
        style={{
          position: 'absolute',
          inset: 0,
          overflow: 'hidden',
          pointerEvents: 'none',
          opacity: playing && !done ? 1 : 0,
          transition: 'opacity 0.6s ease',
        }}
      >
        <div
          ref={hostRef}
          style={{
            position: 'absolute',
            top: 0,
            left: '50%',
            // translateX centers horizontally. scale(1.3) overscans to hide the baked-in
            // letterbox bars. transform-origin top pins the top edge, then a small negative
            // translateY pulls the frame UP so the TOP black bar clips off the top edge too
            // (origin-top alone would only clear the bottom bar). Net: both bars gone, framing
            // sits slightly high — the top-center focus stays in view.
            transform: 'translateX(-50%) translateY(-9%) scale(1.3)',
            transformOrigin: 'top center',
            width: 'max(100%, 177.78vh)',
            height: 'max(100%, 56.25vw)',
          }}
        />
      </div>

      {/* The only visible control: a mute/unmute chip, shown once the trailer is playing. Sits
          above the scrim so it's clickable. */}
      {playing && !done && (
        <button
          type="button"
          onClick={toggleMute}
          aria-label={muted ? 'Unmute trailer' : 'Mute trailer'}
          style={{
            position: 'absolute',
            right: 20,
            bottom: 20,
            zIndex: 2,
            width: 40,
            height: 40,
            borderRadius: '50%',
            border: '1px solid rgba(255,255,255,0.4)',
            background: 'rgba(0,0,0,0.5)',
            color: '#fff',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            cursor: 'pointer',
          }}
        >
          {muted ? (
            // Muted: speaker with an ✕.
            <svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <path d="M3 9v6h4l5 5V4L7 9H3zm13.5 3l2.5 2.5-1 1L15.5 13 13 15.5l-1-1L14.5 12 12 9.5l1-1L15.5 11 18 8.5l1 1L16.5 12z" />
            </svg>
          ) : (
            // Unmuted: speaker with sound waves.
            <svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <path d="M3 9v6h4l5 5V4L7 9H3zm13.5 3c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77z" />
            </svg>
          )}
        </button>
      )}
    </>
  );
}
