import { z } from 'zod';

import { PaginationRequestSchema, PaginationResponseSchema } from './common.ts';

export const ListWorkspacesRequestSchema = PaginationRequestSchema;

export const WorkspaceSummarySchema = z.object({
  id: z.string(),
  name: z.string(),
  path: z.string(),
});

export const ListWorkspacesResponseSchema = z.object({
  workspaces: z.array(WorkspaceSummarySchema),
  pagination: PaginationResponseSchema.optional(),
});
