import { get, post, put, qs } from './api.js';
import { el, renderNav, errorBox, pagination } from './ui.js';

const dataState = { page: 1 };

async function loadUsers() {
  const box = document.getElementById('users');
  try {
    const data = await get('/api/v1/admin/users');
    box.replaceChildren(...(data.results || []).map((u) => el('div', { class: 'row', style: 'padding:6px 0' },
      el('b', {}, `#${u.id} ${u.username}`),
      u.is_admin ? el('span', { class: 'badge' }, '管理员') : null,
      u.is_active ? el('span', { class: 'badge' }, '启用') : el('span', { class: 'badge unread' }, '停用'),
      u.is_active
        ? el('button', { class: 'btn sm danger', onclick: async () => { await post(`/api/v1/admin/users/${u.id}/deactivate`); loadUsers(); } }, '停用')
        : el('button', { class: 'btn sm', onclick: async () => { await post(`/api/v1/admin/users/${u.id}/activate`); loadUsers(); } }, '启用'),
      el('button', { class: 'btn sm', onclick: async () => {
        const pw = prompt('新口令（≥8 位）');
        if (pw) { await post(`/api/v1/admin/users/${u.id}/reset-password`, { password: pw }); alert('已重置'); }
      } }, '重置口令'))));
  } catch (err) {
    if (err.status === 403) { location.href = '/bookmarks'; return; }
    box.replaceChildren(errorBox(err));
  }
}

document.getElementById('user-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const msg = document.getElementById('user-msg');
  msg.className = '';
  try {
    await post('/api/v1/admin/users', {
      username: document.getElementById('u-name').value.trim(),
      password: document.getElementById('u-pass').value,
      is_admin: document.getElementById('u-admin').checked,
    });
    msg.className = 'ok-text';
    msg.textContent = '用户已创建';
    loadUsers();
  } catch (err) {
    msg.className = 'error-text';
    msg.textContent = err.message;
  }
});

async function loadData() {
  const box = document.getElementById('data');
  try {
    const data = await get(`/api/v1/admin/data/bookmarks${qs({ page: dataState.page, per_page: 20 })}`);
    box.replaceChildren(el('table', {},
      el('tr', {}, el('th', {}, 'ID'), el('th', {}, '用户'), el('th', {}, 'URL'), el('th', {}, '标题'), el('th', {}, '状态')),
      ...(data.results || []).map((b) => el('tr', {},
        el('td', {}, b.id), el('td', {}, b.user_id),
        el('td', { style: 'max-width:280px;word-break:break-all' }, b.url),
        el('td', {}, b.title || ''),
        el('td', {}, b.is_archived ? '归档' : '活跃')))));
    document.getElementById('data-pager').replaceChildren(pagination(dataState.page, Math.max(1, Math.ceil((data.total || 0) / 20)), (p) => { dataState.page = p; loadData(); }));
  } catch (err) {
    box.replaceChildren(errorBox(err));
  }
}

async function loadTasks() {
  const box = document.getElementById('tasks');
  try {
    const data = await get('/api/v1/admin/tasks');
    box.replaceChildren(...(data.results || []).map((t) => el('div', { class: 'row', style: 'padding:4px 0' },
      el('span', {}, `#${t.id} 书签${t.bookmark_id} ${t.kind}`),
      el('span', { class: `badge${t.status === 'failed' ? ' unread' : ''}` }, t.status),
      el('span', { class: 'muted' }, `尝试 ${t.attempts} 次`),
      t.last_error ? el('span', { class: 'muted' }, t.last_error) : null)));
    if (!(data.results || []).length) box.replaceChildren(el('div', { class: 'muted' }, '暂无任务'));
  } catch (err) {
    box.replaceChildren(errorBox(err));
  }
}

async function loadAudit() {
  const box = document.getElementById('audit');
  try {
    const data = await get('/api/v1/admin/audit-logs?per_page=20');
    box.replaceChildren(...(data.results || []).map((a) => el('div', { class: 'row', style: 'padding:4px 0' },
      el('span', { class: 'muted' }, a.created_at),
      el('b', {}, a.action),
      el('span', {}, a.username ? `@${a.username}` : ''),
      a.target ? el('span', { class: 'muted' }, a.target) : null)));
    if (!(data.results || []).length) box.replaceChildren(el('div', { class: 'muted' }, '暂无记录'));
  } catch (err) {
    box.replaceChildren(errorBox(err));
  }
}

async function loadSettings() {
  try {
    const s = await get('/api/v1/admin/settings');
    document.getElementById('guest-sharing').checked = s.guest_sharing === 'true' || s.guest_sharing === '1';
  } catch {}
}
document.getElementById('settings-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const msg = document.getElementById('settings-msg');
  msg.className = '';
  try {
    await put('/api/v1/admin/settings', { guest_sharing: document.getElementById('guest-sharing').checked ? 'true' : 'false' });
    msg.className = 'ok-text';
    msg.textContent = '已保存';
  } catch (err) {
    msg.className = 'error-text';
    msg.textContent = err.message;
  }
});

renderNav('admin');
loadUsers();
loadData();
loadTasks();
loadAudit();
loadSettings();
