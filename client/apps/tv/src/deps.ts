/**
 * Single re-export point for the workspace packages the app consumes. Keeps the
 * screens' imports short and makes it obvious which `@medi/*` surface the app
 * depends on.
 */

export {
  ApiClient,
  ApiError,
} from '@medi/api-client';
export type {
  LibraryItem,
  LibraryPage,
  MovieDetail,
  SeriesDetail,
  MediaFile,
  Credit,
  StreamDecision,
} from '@medi/api-client';

export {
  configureTVRemoteControl,
  Page,
  FocusTrap,
  FocusTargetProvider,
  DirectionalOverride,
  FocusGuide,
  useFocusTarget,
  useFocusByName,
  SpatialNavigationScrollView,
  DefaultFocus,
} from '@medi/navigation';
export type { EdgeDirection } from '@medi/navigation';

export {
  HeroBanner,
  Carousel,
  PosterGrid,
  HoverPreview,
  theme,
} from '@medi/ui';
export type { PosterItem } from '@medi/ui';
