import { SERVER_HOSTS } from '@core/constant/servers';
import { fetchWithToken } from '@core/util/fetchWithToken';
import { isTauri } from '@core/util/platform';
import type { TextResponse } from '@core/util/safeFetch';
import {
  createEffect,
  createMemo,
  createSignal,
  mapArray,
  onCleanup,
} from 'solid-js';
import type { MediaItem } from './media-items';

const fileService = SERVER_HOSTS['static-file'];

// The native HTTP transport owns the session. Media elements use the separate
// WebView cookie jar and need a narrowly scoped object grant for streaming.
export function createNativeMediaSource(id: () => string) {
  const [url, setUrl] = createSignal<string>();
  createEffect(() => {
    const fileId = id();
    setUrl(undefined);
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    async function refresh() {
      const grant = await fetchWithToken<TextResponse>(
        `${fileService}/api/file/${encodeURIComponent(fileId)}/presigned-url`,
        { signal: controller.signal }
      );
      if (controller.signal.aborted) return;
      const value = grant.isOk() ? grant.value.body : undefined;
      const expected = new URL(
        `/objects/static-file-storage/file/${fileId}`,
        fileService
      );
      let objectUrl: URL | undefined;
      try {
        if (value) objectUrl = new URL(value);
      } catch {
        // A malformed grant must never be used as a media source.
      }
      const valid =
        objectUrl?.origin === expected.origin &&
        objectUrl.pathname === expected.pathname;
      setUrl(valid ? objectUrl?.href : undefined);
      timer = setTimeout(start, valid ? 55 * 60_000 : 60_000);
    }
    function start() {
      refresh().catch(() => {
        if (!controller.signal.aborted) {
          setUrl(undefined);
          timer = setTimeout(start, 60_000);
        }
      });
    }
    onCleanup(() => {
      controller.abort();
      clearTimeout(timer);
    });
    start();
  });
  return url;
}

export const usesNativeMediaGrants = () =>
  import.meta.env.PROD &&
  isTauri() &&
  fileService === 'https://macro.studio3000.ch/static-file';

export function createPlayableMediaItems(items: () => MediaItem[]) {
  if (!usesNativeMediaGrants()) return items;
  const resolved = mapArray(items, (item) => {
    const grant = createNativeMediaSource(() => item.id);
    return createMemo(() => {
      const src = grant() ?? item.previewSrc;
      return src ? { ...item, src, fullSrc: src, thumbSrc: src } : undefined;
    });
  });
  return createMemo(() => resolved().flatMap((read) => read() ?? []));
}
