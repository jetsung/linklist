import { get, post, put, del } from './api.js';
import { el, renderNav, errorBox } from './ui.js';

// 口令。
document.getElementById('pw-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const msg = document.getElementById('pw-msg');
  msg.className = '';
  try {
    await post('/api/v1/auth/password', {
      current_password: document.getElementById('cur').value,
      new_password: document.getElementById('new').value,
    });
    msg.className = 'ok-text';
    msg.textContent = '口令已更新';
  } catch (err) {
    msg.className = 'error-text';
    msg.textContent = err.message;
  }
});

// Token。
async function loadTokens() {
  const box = document.getElementById('tokens');
  try {
    const data = await get('/api/v1/auth/tokens');
    box.replaceChildren(...(data.results || []).map((t) => el('div', { class: 'row', style: 'padding:4px 0' },
      el('span', {}, `#${t.id} ${t.name || ''}`),
      el('span', { class: 'muted' }, t.created_at || ''),
      el('button', { class: 'btn sm danger', onclick: async () => { await del(`/api/v1/auth/tokens/${t.id}`); loadTokens(); } }, '吊销'))));
  } catch (err) {
    box.replaceChildren(errorBox(err));
  }
}
document.getElementById('token-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const msg = document.getElementById('token-msg');
  try {
    const data = await post('/api/v1/auth/tokens', { name: document.getElementById('token-name').value.trim() });
    msg.textContent = `新 Token（仅显示一次）: ${data.token}`;
    loadTokens();
  } catch (err) {
    msg.textContent = err.message;
  }
});

// 偏好。
async function loadPrefs() {
  try {
    const p = await get('/api/v1/profile');
    document.getElementById('theme').value = p.theme || 'auto';
    document.getElementById('per-page').value = p.items_per_page || 30;
    document.getElementById('favicons').checked = !!p.enable_favicons;
    document.getElementById('previews').checked = !!p.enable_preview_images;
    document.getElementById('autosnap').checked = !!p.enable_automatic_snapshots;
    document.getElementById('pubshare').checked = !!p.enable_public_sharing;
    document.getElementById('custom-css').value = p.custom_css || '';
    if (p.theme === 'dark') document.documentElement.dataset.theme = 'dark';
  } catch (err) {
    if (err.status === 401) location.href = '/login';
  }
}
document.getElementById('pref-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const msg = document.getElementById('pref-msg');
  msg.className = '';
  try {
    await put('/api/v1/profile', {
      theme: document.getElementById('theme').value,
      items_per_page: Number(document.getElementById('per-page').value),
      enable_favicons: document.getElementById('favicons').checked,
      enable_preview_images: document.getElementById('previews').checked,
      enable_automatic_snapshots: document.getElementById('autosnap').checked,
      enable_public_sharing: document.getElementById('pubshare').checked,
      custom_css: document.getElementById('custom-css').value,
    });
    msg.className = 'ok-text';
    msg.textContent = '偏好已保存';
  } catch (err) {
    msg.className = 'error-text';
    msg.textContent = err.message;
  }
});

renderNav('settings');
loadTokens();
loadPrefs();
