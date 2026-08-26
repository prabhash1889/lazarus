import type { RegisteredCommand } from './types';

export interface UsageEntry {
  count: number;
  lastUsedAt: number;
}

export type UsageMap = Record<string, UsageEntry>;

const RECENCY_TIERS: Array<[maxAgeMs: number, boost: number]> = [
  [2 * 60_000, 40],
  [10 * 60_000, 28],
  [60 * 60_000, 18],
  [24 * 60 * 60_000, 8],
];

/**
 * Boost for a recently/frequently used command. With a non-empty query the
 * boost is capped so popularity never outranks a strong textual match.
 */
export function recencyBoost(
  entry: UsageEntry | undefined,
  nowMs: number,
  withQuery: boolean,
): number {
  if (!entry) {
    return 0;
  }
  const age = Math.max(0, nowMs - entry.lastUsedAt);
  let boost = 0;
  for (const [maxAge, tierBoost] of RECENCY_TIERS) {
    if (age <= maxAge) {
      boost = tierBoost;
      break;
    }
  }
  const frequency = Math.min(entry.count, 5);
  const total = boost + frequency;
  return withQuery ? Math.min(total, 15) : total;
}

/**
 * Subsequence fuzzy match scored toward substring and word-boundary hits.
 * Returns the score, or null when `query` is not a subsequence of `text`.
 */
export function fuzzyMatch(query: string, text: string): number | null {
  if (query.length === 0) {
    return 0;
  }
  const lowerQuery = query.toLowerCase();
  const lowerText = text.toLowerCase();
  const exactIndex = lowerText.indexOf(lowerQuery);
  if (exactIndex >= 0) {
    let score = 100 + lowerQuery.length * 4 - exactIndex;
    if (exactIndex === 0 || /[\s/_.:\-]/.test(lowerText[exactIndex - 1] ?? '')) {
      score += 20;
    }
    return score;
  }
  let score = 0;
  let textIndex = 0;
  let previousMatch = -2;
  for (let queryIndex = 0; queryIndex < lowerQuery.length; queryIndex += 1) {
    const char = lowerQuery.charAt(queryIndex);
    const found = lowerText.indexOf(char, textIndex);
    if (found < 0) {
      return null;
    }
    if (found === previousMatch + 1) {
      score += 6;
    } else {
      score -= Math.min(found - previousMatch - 1, 8);
    }
    if (found === 0 || /[\s/_.:\-]/.test(lowerText[found - 1] ?? '')) {
      score += 12;
    }
    previousMatch = found;
    textIndex = found + 1;
  }
  return score;
}

function bestCommandScore(command: RegisteredCommand, query: string): number | null {
  const titleScore = fuzzyMatch(query, command.title);
  let best = titleScore;
  for (const keyword of command.keywords ?? []) {
    const keywordScore = fuzzyMatch(query, keyword);
    if (keywordScore !== null && keywordScore > (best ?? Number.NEGATIVE_INFINITY)) {
      best = keywordScore;
    }
  }
  if (best === null && command.section) {
    best = fuzzyMatch(query, command.section);
  }
  return best;
}

export interface RankedCommand {
  command: RegisteredCommand;
  score: number;
}

/**
 * Ranks available commands against the query. An empty query keeps
 * registration order weighted by recency so the palette opens usefully.
 */
export function searchCommands(
  query: string,
  commands: RegisteredCommand[],
  usage: UsageMap,
  nowMs: number,
): RankedCommand[] {
  const trimmed = query.trim();
  const ranked: RankedCommand[] = [];
  commands.forEach((command, registrationIndex) => {
    if (command.when && !command.when()) {
      return;
    }
    if (trimmed.length === 0) {
      const entry = usage[command.id];
      ranked.push({
        command,
        score: recencyBoost(entry, nowMs, false) - registrationIndex * 0.001,
      });
      return;
    }
    const matchScore = bestCommandScore(command, trimmed);
    if (matchScore === null) {
      return;
    }
    ranked.push({
      command,
      score: matchScore + recencyBoost(usage[command.id], nowMs, true),
    });
  });
  ranked.sort((a, b) => {
    if (b.score !== a.score) {
      return b.score - a.score;
    }
    const byTitle = a.command.title.localeCompare(b.command.title);
    if (byTitle !== 0) {
      return byTitle;
    }
    return a.command.id.localeCompare(b.command.id);
  });
  return ranked;
}
