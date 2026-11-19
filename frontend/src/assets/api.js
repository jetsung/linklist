// fetch API 封装：JSON 请求 + 错误统一处理。
export async function api(path, options = {}) {
  const opts = { credentials: 'same-origin', headers: {}, ...options };
  if (opts.body && typeof opts.body !== 'string') {
    opts.body = JSON.stringify(opts.body);
    opts.headers['Content-Type'] = 'application/json';
  }
  const resp = await fetch(path, opts);
  const text = await resp.text();
  let data = null;
  try { data = text ? JSON.parse(text) : null; } catch { data = { raw: text }; }
  if (!resp.ok) {
    const msg = data && data.error ? (data.error.message || JSON.stringify(data.error)) : `HTTP ${resp.status}`;
    const err = new Error(msg);
    err.status = resp.status;
    throw err;
  }
  return data;
}

export const get = (p) => api(p);
export const post = (p, body) => api(p, { method: 'POST', body: body ?? {} });
export const put = (p, body) => api(p, { method: 'PUT', body });
export const patch = (p, body) => api(p, { method: 'PATCH', body });
export const del = (p) => api(p, { method: 'DELETE' });

export function qs(params) {
  const u = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== null && v !== '') u.set(k, v);
  }
  const s = u.toString();
  return s ? `?${s}` : '';
}
