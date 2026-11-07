/** No HTTP response arrived at all; cleared automatically once the TV answers again. */
export class ConnectionError extends Error {
  override name = 'ConnectionError';
}
export async function request<T>(
  path: string,
  body?: unknown,
  method = body === undefined ? 'GET' : 'POST',
): Promise<T> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), path.includes('/pair/') ? 60_000 : 10_000);
  try {
    let response: Response;
    try {
      response = await fetch(path, {
        method,
        signal: controller.signal,
        headers: body === undefined ? undefined : { 'Content-Type': 'application/json' },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
    } catch (error) {
      // fetch() rejects only when no HTTP answer arrived: the daemon is restarting
      // (e.g. after an Entertainment Area change), the TV is off, or Wi-Fi dropped.
      throw new ConnectionError(
        controller.signal.aborted
          ? 'The TV took too long to answer. Retrying…'
          : 'Can’t reach LG Hue Sync on the TV right now (it may be restarting). Retrying…',
        { cause: error },
      );
    }
    const text = await response.text();
    let data: unknown;
    try {
      data = text ? JSON.parse(text) : {};
    } catch {
      if (response.ok) throw new Error('Invalid server response');
    }
    if (!response.ok) {
      const error =
        typeof data === 'object' && data && 'error' in data
          ? String(data.error)
          : `Request failed (${response.status})`;
      throw new Error(error);
    }
    return data as T;
  } finally {
    clearTimeout(timeout);
  }
}
// Serializes complete settings snapshots; bursts retain only the newest unsent snapshot.
export class SettingsWriter<T> {
  private next: T | undefined;
  private running: Promise<void> | null = null;
  constructor(private write: (value: T) => Promise<void>) {}
  push(value: T) {
    this.next = value;
    return this.drain();
  }
  private drain(): Promise<void> {
    if (this.running) return this.running;
    this.running = (async () => {
      while (this.next !== undefined) {
        const value = this.next;
        this.next = undefined;
        try {
          await this.write(value);
        } catch (error) {
          if (this.next === undefined) this.next = value;
          throw error;
        }
      }
    })().finally(() => {
      this.running = null;
    });
    return this.running;
  }
  async flush() {
    if (this.running) await this.running;
    else if (this.next !== undefined) await this.drain();
  }
  get pending() {
    return this.running !== null;
  }
}
