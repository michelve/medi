/**
 * Typed error for non-2xx API responses. The backend error model is a JSON body
 * `{ "error": { "code", "message" } }` with a matching HTTP status
 * (`docs/.tasks/02-api-contract.md` §Caching & error model).
 */

import type { ApiErrorBody } from './types';

export class ApiError extends Error {
  /** HTTP status code (404, 409, 503, …). */
  readonly status: number;
  /** Stable machine code from the body (e.g. `"not_found"`), or `"http_error"`. */
  readonly code: string;

  constructor(status: number, code: string, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
  }

  /** `true` for a 404 — commonly a not-yet-generated preview/trickplay asset. */
  get isNotFound(): boolean {
    return this.status === 404;
  }

  /** `true` for a 409 — a busy transcode session; the client may retry. */
  get isBusy(): boolean {
    return this.status === 409;
  }

  /** Build an `ApiError` by best-effort parsing the structured error body. */
  static async fromResponse(res: Response): Promise<ApiError> {
    let code = 'http_error';
    let message = `HTTP ${res.status}`;
    try {
      const body = (await res.json()) as Partial<ApiErrorBody>;
      if (body?.error) {
        code = body.error.code ?? code;
        message = body.error.message ?? message;
      }
    } catch {
      // Non-JSON error body (e.g. a static-file 404); keep the defaults.
    }
    return new ApiError(res.status, code, message);
  }
}
