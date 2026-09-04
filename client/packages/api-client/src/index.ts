/**
 * `@medi/api-client` — typed fetch client + response types for the medi REST API.
 * Generated from `docs/.tasks/02-api-contract.md`.
 */

export * from './types';
export { genreSlug, genrePath, titlePath } from './urls';
export { ApiClient } from './client';
export type {
  ApiClientOptions,
  LibraryQuery,
  RequestOptions,
} from './client';
export { ApiError } from './errors';
