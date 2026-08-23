import { z } from 'zod';

/**
 * Shared wire schemas. These mirror the Phase 1 surface: cursor pagination
 * and the error envelope every response can carry.
 */
export const PaginationRequestSchema = z.object({
  pageSize: z.number().int().min(1).max(100).optional(),
  cursor: z.string().optional(),
});

export const PaginationResponseSchema = z.object({
  nextCursor: z.string().optional(),
});

export const ErrorCodeSchema = z.enum([
  'CANCELLED',
  'UNKNOWN',
  'INVALID_ARGUMENT',
  'DEADLINE_EXCEEDED',
  'NOT_FOUND',
  'ALREADY_EXISTS',
  'PERMISSION_DENIED',
  'UNAUTHENTICATED',
  'FAILED_PRECONDITION',
  'INCOMPATIBLE_METHOD_MANIFEST',
  'UNAVAILABLE',
  'INTERNAL',
]);

export const ProtocolErrorSchema = z.object({
  code: ErrorCodeSchema,
  message: z.string(),
});
