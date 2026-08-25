import { z } from 'zod';

export const HealthRequestSchema = z.object({});

export const ServingStatusSchema = z.enum(['SERVING', 'NOT_SERVING']);

export const HealthResponseSchema = z.object({
  status: ServingStatusSchema,
});

export const SubscribeEventsRequestSchema = z.object({});

export const GetInfoRequestSchema = z.object({});

/**
 * Static Host metadata for capability negotiation. `hostVersion` is the
 * running Host's version string; `capabilities` is a feature-name -> support
 * map that only ever grows additively; `startedAtUnixMs` (added in v1.1)
 * stamps when this Host incarnation began serving so clients can render
 * uptime. Peers negotiated at 1.0 receive responses without the field via
 * the declared bridge.
 */
export const GetInfoResponseSchema = z.object({
  hostVersion: z.string(),
  capabilities: z.record(z.string(), z.boolean()),
  startedAtUnixMs: z.number().int().nonnegative().optional(),
});

/**
 * One frame of the server-streamed event feed, exactly as the Host
 * serializes it (camelCase, `type`-tagged). Every subscription opens with an
 * `outage` tombstone and an authoritative `snapshot` frame, then carries only
 * sequenced `live` frames; there is no replay in 1.x, so lagging clients
 * resubscribe for a fresh snapshot.
 */
export const EventFrameSchema = z.discriminatedUnion('type', [
  z.object({
    type: z.literal('outage'),
    outageId: z.string().min(1),
  }),
  z.object({
    type: z.literal('snapshot'),
    workspaces: z.array(
      z.object({
        id: z.string(),
        name: z.string(),
      }),
    ),
    tasks: z.array(
      z.object({
        id: z.string(),
        workspaceId: z.string(),
        title: z.string(),
      }),
    ),
  }),
  z.object({
    type: z.literal('live'),
    sequence: z.number().int().nonnegative(),
  }),
]);
