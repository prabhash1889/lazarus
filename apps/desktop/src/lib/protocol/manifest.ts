/**
 * Codec for the per-method manifest header (`x-lazarus-manifest`) exchanged
 * on every Host response. The wire format is owned by the Rust contract
 * crate (`crates/protocol-rs/src/manifest.rs`): `v1:name=major.minor,...`
 * sorted by name, no duplicates, no trailing comma. This codec mirrors it
 * exactly so TypeScript clients can verify what a Host advertises without
 * a second source of truth.
 */

export interface MethodVersion {
  major: number;
  minor: number;
}

export const MANIFEST_FORMAT_PREFIX = 'v1:';

/** The maximum number of entries one manifest may carry (mirrors Rust). */
export const MAX_MANIFEST_ENTRIES = 128;

export class ManifestCodecError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ManifestCodecError';
  }
}

function parseU32(raw: string): number | null {
  if (!/^\d+$/.test(raw)) {
    return null;
  }
  const value = Number(raw);
  return Number.isSafeInteger(value) ? value : null;
}

function isValidMethodName(name: string): boolean {
  // Same shape the Rust side accepts: dot-separated lowercase identifiers.
  return /^[a-z][a-z0-9]*(\.[a-z][a-zA-Z0-9]*)+$/.test(name);
}

function assertEntryCount(count: number): void {
  if (count > MAX_MANIFEST_ENTRIES) {
    throw new ManifestCodecError(`manifest carries more than ${MAX_MANIFEST_ENTRIES} entries`);
  }
}

/**
 * Decodes an advertised manifest into a name -> version map, preserving
 * insertion order. Malformed input throws `ManifestCodecError`.
 */
export function decodeManifest(raw: string): Map<string, MethodVersion> {
  if (!raw.startsWith(MANIFEST_FORMAT_PREFIX)) {
    throw new ManifestCodecError(
      `manifest must start with ${JSON.stringify(MANIFEST_FORMAT_PREFIX)}`,
    );
  }
  const body = raw.slice(MANIFEST_FORMAT_PREFIX.length);
  const decoded = new Map<string, MethodVersion>();
  if (body === '') {
    throw new ManifestCodecError('manifest carries no entries');
  }
  for (const entry of body.split(',')) {
    const separator = entry.indexOf('=');
    if (separator < 0) {
      throw new ManifestCodecError(`manifest entry ${JSON.stringify(entry)} has no version`);
    }
    const name = entry.slice(0, separator);
    const version = entry.slice(separator + 1);
    if (!isValidMethodName(name)) {
      throw new ManifestCodecError(`invalid method name ${JSON.stringify(name)}`);
    }
    if (decoded.has(name)) {
      throw new ManifestCodecError(`duplicate method name ${JSON.stringify(name)}`);
    }
    const [majorRaw, minorRaw, ...extra] = version.split('.');
    if (majorRaw === undefined || minorRaw === undefined || extra.length > 0) {
      throw new ManifestCodecError(`version of ${JSON.stringify(name)} must be major.minor`);
    }
    const major = parseU32(majorRaw);
    const minor = parseU32(minorRaw);
    if (major === null || minor === null) {
      throw new ManifestCodecError(`version of ${JSON.stringify(name)} must be numeric`);
    }
    decoded.set(name, { major, minor });
    assertEntryCount(decoded.size);
  }
  return decoded;
}

/**
 * Encodes a manifest in canonical form: entries sorted by name so both
 * peers render byte-identical strings for the same negotiation state.
 */
export function encodeManifest(entries: ReadonlyMap<string, MethodVersion>): string {
  const names = [...entries.keys()].sort();
  assertEntryCount(names.length);
  const parts = names.map((name) => {
    const version = entries.get(name);
    if (version === undefined) {
      throw new ManifestCodecError(`no version recorded for ${name}`);
    }
    return `${name}=${version.major}.${version.minor}`;
  });
  if (parts.length === 0) {
    throw new ManifestCodecError('a manifest must carry at least one entry');
  }
  return MANIFEST_FORMAT_PREFIX + parts.join(',');
}
