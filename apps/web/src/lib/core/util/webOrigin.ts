import { isTauri } from './platform';

export function getWebOrigin(): string {
  if (isTauri()) {
    if (import.meta.env.MODE === 'production') return 'https://macro.studio3000.ch';
    return import.meta.env.MODE === 'development'
      ? 'https://dev.macro.com'
      : 'https://macro.com';
  }
  return window.location.origin;
}
