import { get, post, patch, del, qs } from './api.js';
import { el, renderNav, tagChips, pagination, errorBox } from './ui.js';

const state = { q: '', filter: '', tag: '', page: 1, perPage: 30, total: 0, tags: [], selected: new Set() };

function totalPages() { return Math.max(1, Math.ceil(state.total / state.perPage)); }

async function loadTags() {
  try {
    const data = await get('/api/v1/bookmarks/tags');
    state.tags = (data.results || data).map((t) => (typeof t === 'string' ? t : t.name));
    renderTags();
  } catch {}
}

function renderTags() {
  const box = document.getElementById('tags');
  box.replaceChildren(tagChips(state.tags, state.tag, (t) => {
    state.tag = state.tag === t ? '' : t;
    state.page = 1;
    renderTags();
    load();
  }));
}

function bookmarkRow(b) {
  const row = el('div', { class: 'bookmark-item' });
  const cb = el('input', { type: 'checkbox' });
  cb.checked = state.selected.has(b.id);
  cb.addEventListener('change', () => {
    if (cb.checked) state.selected.add(b.id); else state.selected.delete(b.id);
    updateBulkBar();
  });
  const favicon = el('img', { class: 'favicon', src: `https://icons.duckduckgo.com/ip3/${safeHost(b.url)}.ico`, onerror: () => favicon.remove() });
  const body = el('div', { class: 'body' });
  body.append(el('a', { class: 'title', href: `/detail?id=${b.id}` }, b.title || b.url));
  body.append(el('div', { class: 'url' }, b.url));
  if (b.description) body.append(el('div', { class: 'desc' }, b.description));
  const tags = el('div', {}, ...(b.tags || []).map((t) => el('span', { class: 'tag' }, typeof t === 'string' ? t : t.name)));
  if ((b.tags || []).length) body.append(tags);
  const actions = el('div', { class: 'actions' });
  if (b.is_unread) actions.append(el('span', { class: 'badge unread' }, '未读'));
  if (b.is_archived) actions.append(el('span', { class: 'badge' }, '已归档'));
  actions.append(el('button', {
    class: 'btn sm',
    onclick: async () => { await patch(`/api/v1/bookmarks/${b.id}`, { is_archived: !b.is_archived }); load(); },
  }, b.is_archived ? '取消归档' : '归档'));
  actions.append(el('button', { class: 'btn sm danger', onclick: async () => { if (confirm('删除该书签？')) { await del(`/api/v1/bookmarks/${b.id}`); load(); } } }, '删除'));
  row.append(cb, favicon, body, actions);
  return row;
}

function safeHost(url) {
  try { return new URL(url).hostname; } catch { return ''; }
}

function updateBulkBar() {
  document.getElementById('bulk-bar').style.display = state.selected.size ? 'flex' : 'none';
  document.getElementById('sel-count').textContent = state.selected.size;
}

async function bulkCall(path, body) {
  const ids = [...state.selected];
  for (const id of ids) {
    await post(path.replace('{id}', id), body).catch((e) => console.error(e));
  }
  state.selected.clear();
  updateBulkBar();
  load();
}

async function load() {
  const list = document.getElementById('list');
  list.replaceChildren(el('div', { class: 'muted' }, '加载中…'));
  try {
    const data = await get(`/api/v1/bookmarks${qs({ q: state.q, filter: state.filter, tag: state.tag, page: state.page, per_page: state.perPage })}`);
    state.total = data.total ?? (data.results || []).length;
    list.replaceChildren(...(data.results || []).map(bookmarkRow));
    if (!(data.results || []).length) list.replaceChildren(el('div', { class: 'muted' }, '暂无书签'));
    const pager = document.getElementById('pager');
    pager.replaceChildren(pagination(state.page, totalPages(), (p) => { state.page = p; load(); }));
  } catch (err) {
    if (err.status === 401) { location.href = '/login'; return; }
    list.replaceChildren(errorBox(err));
  }
}

document.getElementById('search-form').addEventListener('submit', (e) => {
  e.preventDefault();
  state.q = document.getElementById('q').value.trim();
  state.filter = document.getElementById('filter').value;
  state.page = 1;
  load();
});

// 新建对话框。
const dialog = document.getElementById('new-dialog');
document.getElementById('new-btn').addEventListener('click', () => dialog.showModal());
document.getElementById('n-cancel').addEventListener('click', () => dialog.close());
document.getElementById('new-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const msg = document.getElementById('n-msg');
  msg.textContent = '';
  const tags = document.getElementById('n-tags').value.split(',').map((s) => s.trim()).filter(Boolean);
  try {
    const created = await post('/api/v1/bookmarks', {
      url: document.getElementById('n-url').value.trim(),
      title: document.getElementById('n-title').value.trim() || null,
      description: document.getElementById('n-desc').value.trim() || null,
      is_shared: document.getElementById('n-shared').checked,
    });
    if (tags.length) await patch(`/api/v1/bookmarks/${created.id}/tags`, { tags });
    dialog.close();
    loadTags();
    load();
  } catch (err) {
    msg.textContent = err.message;
  }
});

// 批量操作。
document.getElementById('bulk-add').addEventListener('click', () => bulkCall('/api/v1/bookmarks/bulk/tags/add', { ids: [...state.selected], tags: document.getElementById('bulk-tags').value.split(',').map((s) => s.trim()).filter(Boolean) }));
document.getElementById('bulk-remove').addEventListener('click', () => bulkCall('/api/v1/bookmarks/bulk/tags/remove', { ids: [...state.selected], tags: document.getElementById('bulk-tags').value.split(',').map((s) => s.trim()).filter(Boolean) }));
document.getElementById('bulk-read').addEventListener('click', () => bulkCall('/api/v1/bookmarks/bulk/mark-read', { ids: [...state.selected] }));
document.getElementById('bulk-unread').addEventListener('click', () => bulkCall('/api/v1/bookmarks/bulk/mark-unread', { ids: [...state.selected] }));
document.getElementById('bulk-delete').addEventListener('click', () => { if (confirm('删除所选书签？')) bulkCall('/api/v1/bookmarks/bulk/delete', { ids: [...state.selected] }); });

renderNav('bookmarks');
loadTags();
load();
