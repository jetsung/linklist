// fetch API 封装：JSON 请求 + 错误统一处理。
export async function api(path: string, options: RequestInit = {}): Promise<any> {
  const opts: RequestInit = { credentials: 'same-origin', headers: {}, ...options };
  if (opts.body && typeof opts.body !== 'string') {
    opts.body = JSON.stringify(opts.body);
    (opts.headers as Record<string, string>)['Content-Type'] = 'application/json';
  }
  const resp = await fetch(path, opts);
  const text = await resp.text();
  let data: any = null;
  try { data = text ? JSON.parse(text) : null; } catch { data = { raw: text }; }
  if (!resp.ok) {
    const msg = data && data.error ? (data.error.message || JSON.stringify(data.error)) : `HTTP ${resp.status}`;
    const err = new Error(msg) as Error & { status: number };
    err.status = resp.status;
    throw err;
  }
  return data;
}

export const get = (p: string) => api(p);
export const post = (p: string, body?: unknown) => api(p, { method: 'POST', body: body ?? {} });
export const put = (p: string, body?: unknown) => api(p, { method: 'PUT', body });
export const patch = (p: string, body?: unknown) => api(p, { method: 'PATCH', body });
export const del = (p: string) => api(p, { method: 'DELETE' });

export function qs(params: Record<string, unknown>): string {
  const u = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== null && v !== '') u.set(k, String(v));
  }
  const s = u.toString();
  return s ? `?${s}` : '';
}
