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
