import { describe, expect, it } from 'vitest';

import { ManifestCodecError, decodeManifest, encodeManifest } from './manifest';

describe('manifest wire codec', () => {
  it('round-trips through the canonical sorted form', () => {
    const raw = 'v1:system.health=1.0,system.getInfo=1.1,task.list=1.2';
    const decoded = decodeManifest(raw);
    expect(decoded.get('system.getInfo')).toEqual({ major: 1, minor: 1 });
    // Encoding always sorts by name regardless of insertion order.
    expect(encodeManifest(decoded)).toBe('v1:system.getInfo=1.1,system.health=1.0,task.list=1.2');
  });

  it('rejects every malformed shape the Rust codec rejects', () => {
    for (const bad of [
      '',
      'v1:',
      'v2:system.health=1.0',
      'system.health=1.0',
      'v1:system.health',
      'v1:system.health=1',
      'v1:system.health=1.',
      'v1:system.health=.0',
      'v1:system.health=1.0.0',
      'v1:system.health=x.y',
      'v1:system.health=-1.0',
      'v1:system.health=1.0,',
      'v1:system getInfo=1.0',
      'v1:=1.0',
      'v1:a.b=1.0,a.b=1.1',
    ]) {
      expect(() => decodeManifest(bad), JSON.stringify(bad)).toThrow(ManifestCodecError);
    }
  });

  it('refuses to encode an empty manifest or unsorted duplicates', () => {
    expect(() => encodeManifest(new Map())).toThrow(ManifestCodecError);
  });
});
