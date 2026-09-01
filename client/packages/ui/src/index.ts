/**
 * `@medi/ui` — shared TV components: `HeroBanner`, `Carousel`, `PosterGrid`, and
 * the xstate-governed `HoverPreview` (the Netflix-style delayed hover-to-play).
 */

export { HeroBanner } from './HeroBanner';
export type { HeroBannerProps } from './HeroBanner';

export { Carousel } from './Carousel';
export type { CarouselProps } from './Carousel';

export { PosterGrid } from './PosterGrid';
export type { PosterGridProps } from './PosterGrid';

export { PosterCard } from './PosterCard';
export type { PosterCardProps } from './PosterCard';

export * from './hover-preview';

export { theme } from './theme';
export type { Theme } from './theme';
export type { PosterItem } from './types';
