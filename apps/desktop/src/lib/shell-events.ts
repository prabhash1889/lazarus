export const NAVIGATE_EVENT = 'shell://navigate';
export const DEEP_LINK_EVENT = 'deep-link://open';

const NAVIGABLE_ROUTES = {
  '/': '/',
  '/host-status': '/host-status',
} as const;

export type NavTarget = keyof typeof NAVIGABLE_ROUTES;

export function isNavTarget(path: string): path is NavTarget {
  return path in NAVIGABLE_ROUTES;
}
