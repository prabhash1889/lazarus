import { z } from 'zod';

import { PaginationRequestSchema, PaginationResponseSchema } from './common.ts';

export const ListTasksRequestSchema = PaginationRequestSchema;

export const TaskStatusSchema = z.enum(['PENDING', 'RUNNING', 'COMPLETED', 'FAILED', 'CANCELLED']);

export const TaskSummarySchema = z.object({
  id: z.string(),
  title: z.string(),
  status: TaskStatusSchema,
});

export const ListTasksResponseSchema = z.object({
  tasks: z.array(TaskSummarySchema),
  pagination: PaginationResponseSchema.optional(),
  /**
   * v1.1+: Unix epoch milliseconds at which this page was computed.
   * Stripped by the declared 1.0 bridge for older peers.
   */
  servedAtUnixMs: z.number().int().nonnegative().optional(),
});

/**
 * Task layout records: the durable per-Task shell state (open tabs plus the
 * serialized tile-canvas split tree) that the Desktop restores verbatim.
 * The Host persists the document opaquely - it never interprets tile
 * semantics - so the payload is a bounded JSON-object string here and the
 * schema lives on the client side of the boundary.
 */
const TASK_ID_MAX = 128;
const TASK_LAYOUT_JSON_MAX = 262_144;

export const GetTaskLayoutRequestSchema = z.object({
  taskId: z.string().min(1).max(TASK_ID_MAX),
});

export const GetTaskLayoutResponseSchema = z.object({
  taskId: z.string().min(1).max(TASK_ID_MAX),
  /** The persisted JSON document; absent when the task has no layout yet. */
  layoutJson: z.string().min(1).max(TASK_LAYOUT_JSON_MAX).optional(),
  /**
   * Monotonic record revision. Zero means "no record exists"; every put
   * that lands produces the next positive revision.
   */
  revision: z.number().int().nonnegative(),
});

export const PutTaskLayoutRequestSchema = z.object({
  taskId: z.string().min(1).max(TASK_ID_MAX),
  layoutJson: z.string().min(1).max(TASK_LAYOUT_JSON_MAX),
  /**
   * Optimistic-concurrency guard: when present, the write applies only if
   * it names the current revision; otherwise the Host rejects with
   * FAILED_PRECONDITION and the caller reloads before retrying.
   */
  expectedRevision: z.number().int().nonnegative().optional(),
});

export const PutTaskLayoutResponseSchema = z.object({
  taskId: z.string().min(1).max(TASK_ID_MAX),
  revision: z.number().int().min(1),
});
