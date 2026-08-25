import {
  ErrorCodeSchema,
  ProtocolErrorSchema,
  METHODS,
  methodByName,
  resolveMethodSupport,
  type MethodDefinition,
  type MethodSupport,
} from '@lazarus/protocol-ts';

import { IncompatibleManifestError, ProtocolCallError, RequestCancelledError } from './errors';
import { decodeManifest } from './manifest';

/** Mirrors the Rust bridge's `IpcResponse` (camelCase over Tauri IPC). */
export interface IpcResponse {
  ok: boolean;
  status: number | null;
  manifest: string | null;
  body: string | null;
  error: {
    code: string;
    message: string;
    retryable: boolean;
  } | null;
}

export interface UnaryArgs {
  requestId: number;
  path: string;
  httpMethod?: string;
  payload?: string | null;
}

/**
 * The transport the client speaks through. The production implementation is
 * the Rust IPC bridge (`host_ipc_request`); tests substitute fakes.
 */
export interface HostTransport {
  unary(args: UnaryArgs): Promise<IpcResponse>;
  cancel(requestId: number): Promise<unknown>;
}

/** One negotiated method's outcome, as rendered on the status surface. */
export interface NegotiatedMethodSummary {
  name: string;
  support: MethodSupport;
  /** major.minor when supported. */
  version: string | null;
  /** The substitute method when degraded to a fallback. */
  fallback: string | null;
}

export interface CallOptions {
  signal?: AbortSignal;
}

export interface HostProbe {
  hostVersion: string;
  capabilities: Array<{ name: string; enabled: boolean }>;
  startedAtUnixMs: number | null;
  servingStatus: 'SERVING' | 'NOT_SERVING';
  methods: NegotiatedMethodSummary[];
}

let nextRequestId = 1;

function freshRequestId(): number {
  const id = nextRequestId;
  nextRequestId += 1;
  return id;
}

function parseErrorEnvelope(raw: string | null): { code: string; message: string } | null {
  if (raw === null) {
    return null;
  }
  try {
    const envelope = ProtocolErrorSchema.safeParse(JSON.parse(raw));
    return envelope.success ? envelope.data : null;
  } catch {
    return null;
  }
}

const CANONICAL_CODES: ReadonlySet<string> = new Set(
  // The enum schema exposes its members through `options`.
  ErrorCodeSchema.options,
);

/**
 * Maps one failed `IpcResponse` onto a thrown typed error. A `null` status
 * means the failure happened before HTTP existed (dial failure, timeout,
 * cancellation): transport layer. Anything else is the Host's own answer.
 */
function throwForFailedResponse(response: IpcResponse): never {
  const envelope = response.error ?? parseErrorEnvelope(response.body);
  if (!envelope) {
    throw new ProtocolCallError({
      code: 'INTERNAL',
      message:
        response.status === null
          ? 'the local transport failed without a typed reason'
          : `the Host returned an unexpected error (HTTP ${response.status})`,
      layer: response.status === null ? 'transport' : 'host',
    });
  }
  const layer = response.status === null ? 'transport' : 'host';
  throw new ProtocolCallError({
    // The bridge only emits canonical codes; anything else is off-contract
    // and degrades to INTERNAL instead of being trusted.
    code: CANONICAL_CODES.has(envelope.code)
      ? (envelope.code as ProtocolCallError['code'])
      : 'INTERNAL',
    message: envelope.message,
    layer,
  });
}

/**
 * Verifies the Host's advertised manifest against this client's registry
 * bindings. Throws `IncompatibleManifestError` naming every required method
 * this client needs that the peer cannot serve.
 */
export function negotiatePeerManifest(advertisedRaw: string): NegotiatedMethodSummary[] {
  const peer = decodeManifest(advertisedRaw);
  const summaries: NegotiatedMethodSummary[] = [];
  const offenders: string[] = [];
  for (const method of METHODS) {
    const support = resolveMethodSupport(method, peer);
    if (support === 'supported') {
      const served = peer.get(method.name) ?? method.version;
      summaries.push({
        name: method.name,
        support,
        version: `${served.major}.${served.minor}`,
        fallback: null,
      });
    } else if (support === 'fallback' && method.fallback !== undefined) {
      summaries.push({ name: method.name, support, version: null, fallback: method.fallback });
    } else {
      summaries.push({ name: method.name, support, version: null, fallback: null });
      if (!method.optional) {
        offenders.push(method.name);
      }
    }
  }
  if (offenders.length > 0) {
    throw new IncompatibleManifestError(offenders);
  }
  return summaries;
}

/**
 * Maps registered methods onto their HTTP routes on the Host surface.
 * Unknown methods fail loudly at call time rather than silently.
 */
const RPC_ROUTES: Record<string, { path: string; verb: 'GET' | 'POST' }> = {
  'system.getInfo': { path: '/system/info', verb: 'GET' },
  'system.health': { path: '/system/health', verb: 'GET' },
  'system.subscribeEvents': { path: '/system/events', verb: 'GET' },
  'workspace.list': { path: '/workspaces', verb: 'GET' },
  'task.list': { path: '/tasks', verb: 'GET' },
  'process.start': { path: '/process/start', verb: 'POST' },
  'process.stop': { path: '/process/stop', verb: 'POST' },
  'process.list': { path: '/process/list', verb: 'GET' },
  'process.output': { path: '/process/output', verb: 'GET' },
  'process.resume': { path: '/process/resume', verb: 'POST' },
};

function routeOf(method: MethodDefinition): { path: string; verb: 'GET' | 'POST' } {
  const route = RPC_ROUTES[method.name];
  if (route === undefined) {
    throw new ProtocolCallError({
      code: 'INVALID_ARGUMENT',
      message: `${method.name} has no known Host route`,
      layer: 'transport',
    });
  }
  return route;
}

/**
 * The protocol client used across the Desktop: every call validates its
 * request against the TypeScript/Zod contract registry before sending and
 * decodes the response through the same registry after verifying the
 * Host's advertised manifest per method. Cancellation aborts end to end
 * while the call is in flight.
 */
export class LazarusProtocolClient {
  /** The manifest advertised by the most recent successful response. */
  lastAdvertisedManifest: string | null = null;

  constructor(private readonly transport: HostTransport) {}

  /**
   * Executes one unary RPC, returning the contract-decoded response
   * payload. Throws `RequestCancelledError`, `ProtocolCallError`, or
   * `IncompatibleManifestError`.
   */
  async call(methodName: string, input: unknown, options?: CallOptions): Promise<unknown> {
    const method = methodByName(methodName);
    if (method === undefined || method.kind !== 'unary') {
      throw new ProtocolCallError({
        code: 'INVALID_ARGUMENT',
        message: `${methodName} is not a registered unary method`,
        layer: 'transport',
      });
    }
    const requestPayload = method.request.parse(input);

    const requestId = freshRequestId();
    if (options?.signal?.aborted) {
      throw new RequestCancelledError();
    }

    let cancelled = false;
    let removeAbortListener: (() => void) | undefined;
    if (options?.signal) {
      const onAbort = () => {
        cancelled = true;
        void this.transport.cancel(requestId).catch(() => {});
      };
      options.signal.addEventListener('abort', onAbort, { once: true });
      removeAbortListener = () => options.signal?.removeEventListener('abort', onAbort);
    }

    try {
      const route = routeOf(method);
      const payloadText =
        Object.keys(requestPayload as Record<string, unknown>).length > 0
          ? JSON.stringify(requestPayload)
          : null;
      const response = await this.transport.unary({
        requestId,
        path: route.path,
        httpMethod: route.verb,
        payload: payloadText,
      });

      if (cancelled) {
        throw new RequestCancelledError();
      }
      if (!response.ok) {
        throwForFailedResponse(response);
      }
      if (typeof response.manifest !== 'string') {
        throw new ProtocolCallError({
          code: 'INTERNAL',
          message: 'the Host response did not advertise its method manifest',
          layer: 'host',
        });
      }
      this.lastAdvertisedManifest = response.manifest;

      let parsedBody: unknown;
      try {
        parsedBody = JSON.parse(response.body ?? '');
      } catch {
        throw new ProtocolCallError({
          code: 'INTERNAL',
          message: 'the Host response body was not valid JSON',
          layer: 'host',
        });
      }
      return method.response.parse(parsedBody);
    } finally {
      removeAbortListener?.();
    }
  }

  /**
   * Executes `system.getInfo` and `system.health` with full contract
   * decoding, verifying compatibility against the advertised manifest and
   * producing everything the status surface renders.
   */
  async probeHost(options?: CallOptions): Promise<HostProbe> {
    const info = (await this.call('system.getInfo', {}, options)) as {
      hostVersion: string;
      capabilities: Record<string, boolean>;
      startedAtUnixMs?: number;
    };
    const health = (await this.call('system.health', {}, options)) as {
      status: 'SERVING' | 'NOT_SERVING';
    };

    // Negotiation runs once more purely for the summary; the calls above
    // already proved compatibility or they would have thrown.
    const methods =
      this.lastAdvertisedManifest !== null
        ? negotiatePeerManifest(this.lastAdvertisedManifest)
        : [];

    return {
      hostVersion: info.hostVersion,
      capabilities: Object.entries(info.capabilities)
        .map(([name, enabled]) => ({ name, enabled }))
        .sort((left, right) => left.name.localeCompare(right.name)),
      startedAtUnixMs: info.startedAtUnixMs ?? null,
      servingStatus: health.status,
      methods,
    };
  }
}
