import { afterEach, describe, expect, it, vi } from 'vitest';

import { IncompatibleManifestError, ProtocolCallError, RequestCancelledError } from './errors';
import {
  LazarusProtocolClient,
  negotiatePeerManifest,
  type HostTransport,
  type IpcResponse,
} from './client';
import { encodeManifest } from './manifest';

function okResponse(body: unknown, manifest: string): IpcResponse {
  return {
    ok: true,
    status: 200,
    manifest,
    body: JSON.stringify(body),
    error: null,
  };
}

const HOST_MANIFEST = encodeManifest(
  new Map([
    ['process.list', { major: 1, minor: 0 }],
    ['process.output', { major: 1, minor: 0 }],
    ['process.resume', { major: 1, minor: 0 }],
    ['process.start', { major: 1, minor: 0 }],
    ['process.stop', { major: 1, minor: 0 }],
    ['system.getInfo', { major: 1, minor: 1 }],
    ['system.health', { major: 1, minor: 0 }],
    ['system.subscribeEvents', { major: 1, minor: 0 }],
    ['task.list', { major: 1, minor: 2 }],
    ['workspace.list', { major: 1, minor: 0 }],
  ]),
);

interface CallRecord {
  requestId: number;
  path: string;
  httpMethod?: string;
  payload?: string | null;
}

function fakeTransport(handler: (call: CallRecord) => IpcResponse | Promise<IpcResponse>): {
  transport: HostTransport;
  calls: CallRecord[];
  cancelledIds: number[];
} {
  const calls: CallRecord[] = [];
  const cancelledIds: number[] = [];
  return {
    calls,
    cancelledIds,
    transport: {
      async unary(args) {
        calls.push(args);
        return handler(args);
      },
      async cancel(requestId) {
        cancelledIds.push(requestId);
        return true;
      },
    },
  };
}

describe('manifest negotiation', () => {
  it('summarizes every method at the shared versions', () => {
    const summary = negotiatePeerManifest(HOST_MANIFEST);
    const taskList = summary.find((entry) => entry.name === 'task.list');
    expect(taskList).toEqual({
      name: 'task.list',
      support: 'supported',
      version: '1.2',
      fallback: null,
    });
  });

  it('tolerates newer additive minors as supported peers', () => {
    const summary = negotiatePeerManifest(HOST_MANIFEST.replace('task.list=1.2', 'task.list=1.9'));
    expect(summary.find((entry) => entry.name === 'task.list')?.support).toBe('supported');
  });

  it('names every unsupported required method when the peer cannot serve them', () => {
    // process.start jumps majors - no bridge can save that peer.
    const incompatible = HOST_MANIFEST.replace('process.start=1.0', 'process.start=2.0');
    let caught: unknown;
    try {
      negotiatePeerManifest(incompatible);
    } catch (error) {
      caught = error;
    }
    expect(caught).toBeInstanceOf(IncompatibleManifestError);
    expect((caught as IncompatibleManifestError).offenders).toContain('process.start');
  });
});

describe('LazarusProtocolClient', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('decodes a successful response through the contract schema', async () => {
    const { transport } = fakeTransport(() =>
      okResponse(
        { hostVersion: '9.9.9', capabilities: { events: true }, extraFutureField: 1 },
        HOST_MANIFEST,
      ),
    );
    const client = new LazarusProtocolClient(transport);
    const result = (await client.call('system.getInfo', {})) as {
      hostVersion: string;
      capabilities: Record<string, boolean>;
    };
    expect(result.hostVersion).toBe('9.9.9');
    expect(client.lastAdvertisedManifest).toBe(HOST_MANIFEST);
  });

  it('sends the mapped route and verb for POST methods', async () => {
    const { transport, calls } = fakeTransport((call) => {
      const parsed = JSON.parse(call.payload ?? '{}') as { processId: string };
      return okResponse({ processId: parsed.processId, status: 'RUNNING' }, HOST_MANIFEST);
    });
    const client = new LazarusProtocolClient(transport);
    await client.call('process.start', {
      processId: '0198e550-c9be-7000-8000-000000000001',
      program: 'fake-agent',
      dataDir: '/tmp/lazarus',
      runMode: 'PIPED',
      args: [],
    });
    expect(calls[0]?.httpMethod).toBe('POST');
    expect(calls[0]?.path).toBe('/process/start');
    expect(calls[0]?.requestId).toBeGreaterThan(0);
  });

  it('surfaces host rejections as typed protocol errors', async () => {
    const { transport } = fakeTransport(() => ({
      ok: false,
      status: 401,
      manifest: null,
      body: null,
      error: {
        code: 'UNAUTHENTICATED',
        message: 'missing or invalid local token',
        retryable: false,
      },
    }));
    const client = new LazarusProtocolClient(transport);
    const error = await client.call('system.getInfo', {}).catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(ProtocolCallError);
    const typed = error as ProtocolCallError;
    expect(typed.code).toBe('UNAUTHENTICATED');
    expect(typed.retryable).toBe(false);
    expect(typed.layer).toBe('host');
  });

  it('maps transport failures to the transport layer with canonical retryability', async () => {
    const { transport } = fakeTransport(() => ({
      ok: false,
      status: null,
      manifest: null,
      body: null,
      error: {
        code: 'DEADLINE_EXCEEDED',
        message: 'the Host did not answer within the request budget',
        retryable: true,
      },
    }));
    const client = new LazarusProtocolClient(transport);
    const typed = (await client
      .call('system.getInfo', {})
      .catch((caught: unknown) => caught)) as ProtocolCallError;
    expect(typed.code).toBe('DEADLINE_EXCEEDED');
    expect(typed.retryable).toBe(true);
    expect(typed.layer).toBe('transport');
  });

  it('degrades off-contract failure envelopes to INTERNAL instead of trusting them', async () => {
    const { transport } = fakeTransport(() => ({
      ok: false,
      status: 500,
      manifest: null,
      body: '<html>boom</html>',
      error: null,
    }));
    const client = new LazarusProtocolClient(transport);
    const typed = (await client
      .call('system.getInfo', {})
      .catch((caught: unknown) => caught)) as ProtocolCallError;
    expect(typed.code).toBe('INTERNAL');
  });

  it('rejects request payloads that violate the method contract before sending', async () => {
    const { transport, calls } = fakeTransport(() => okResponse({}, HOST_MANIFEST));
    const client = new LazarusProtocolClient(transport);
    await expect(client.call('task.list', { pageSize: 0 })).rejects.toBeInstanceOf(Error);
    expect(calls).toHaveLength(0);
  });

  it('rejects unknown or streaming methods on the unary surface', async () => {
    const { transport } = fakeTransport(() => okResponse({}, HOST_MANIFEST));
    const client = new LazarusProtocolClient(transport);
    await expect(client.call('no.such.method', {})).rejects.toBeInstanceOf(ProtocolCallError);
    await expect(client.call('system.subscribeEvents', {})).rejects.toBeInstanceOf(
      ProtocolCallError,
    );
  });

  it('cancels before sending without touching the transport', async () => {
    const { transport, calls, cancelledIds } = fakeTransport(() => okResponse({}, HOST_MANIFEST));
    const controller = new AbortController();
    controller.abort();
    const client = new LazarusProtocolClient(transport);
    await expect(
      client.call('system.getInfo', {}, { signal: controller.signal }),
    ).rejects.toBeInstanceOf(RequestCancelledError);
    expect(calls).toHaveLength(0);
    expect(cancelledIds).toHaveLength(0);
  });

  it('cancels in flight end to end through the transport bridge', async () => {
    let release!: (response: IpcResponse) => void;
    const { transport, calls, cancelledIds } = fakeTransport(
      () =>
        new Promise<IpcResponse>((resolve) => {
          release = resolve;
        }),
    );
    const controller = new AbortController();
    const client = new LazarusProtocolClient(transport);
    const pending = client.call('system.getInfo', {}, { signal: controller.signal });

    await Promise.resolve();
    controller.abort();
    expect(cancelledIds).toEqual(calls.map((call) => call.requestId));

    release(okResponse({ hostVersion: 'late', capabilities: {} }, HOST_MANIFEST));
    await expect(pending).rejects.toBeInstanceOf(RequestCancelledError);
  });

  it('probeHost composes info and health into the renderable snapshot', async () => {
    let getInfoCalls = 0;
    const { transport } = fakeTransport((call) => {
      if (call.path === '/system/info') {
        getInfoCalls += 1;
        return okResponse(
          {
            hostVersion: '1.2.3',
            capabilities: { events: true },
            startedAtUnixMs: 1756100000000,
          },
          HOST_MANIFEST,
        );
      }
      return okResponse({ status: 'SERVING' }, HOST_MANIFEST);
    });
    const client = new LazarusProtocolClient(transport);
    const probe = await client.probeHost();
    expect(getInfoCalls).toBe(1);
    expect(probe.hostVersion).toBe('1.2.3');
    expect(probe.servingStatus).toBe('SERVING');
    expect(probe.startedAtUnixMs).toBe(1756100000000);
    expect(probe.capabilities).toEqual([{ name: 'events', enabled: true }]);
    expect(probe.methods.map((method) => method.name)).toContain('task.list');
  });
});
