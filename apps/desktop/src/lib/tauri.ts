export function invokeCommand<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const tauri = window.__TAURI__;
  if (!tauri) {
    return Promise.reject(
      new Error(
        'Tauri bridge unavailable. Launch the app through the desktop shell (pnpm dev:desktop) instead of a plain browser tab.',
      ),
    );
  }
  return tauri.core.invoke<T>(cmd, args);
}

export async function listenToEvent<T>(
  event: string,
  handler: (payload: T) => void,
): Promise<() => void> {
  const tauri = window.__TAURI__;
  if (!tauri) {
    return () => {};
  }
  const unlisten = await tauri.event.listen<T>(event, (evt) => handler(evt.payload));
  return unlisten;
}
