import { get, qs } from './api.js';
import { el, renderNav, pagination, errorBox } from './ui.js';

const state = { q: '', tag: '', page: 1, perPage: 30, total: 0 };

async function load() {
  const list = document.getElementById('list');
  list.replaceChildren(el('div', { class: 'muted' }, '加载中…'));
  try {
    const data = await get(`/api/v1/shared${qs({ q: state.q, tag: state.tag, page: state.page, per_page: state.perPage })}`);
    state.total = data.total ?? 0;
    list.replaceChildren(...(data.results || []).map((b) => el('div', { class: 'bookmark-item' },
      el('div', { class: 'body' },
        el('a', { class: 'title', href: b.url, target: '_blank' }, b.title || b.url),
        el('div', { class: 'url' }, b.url),
        el('div', { class: 'muted', style: 'font-size:12.5px' }, `来自用户 #${b.user_id}`))),
    ));
    if (!(data.results || []).length) list.replaceChildren(el('div', { class: 'muted' }, '暂无共享书签'));
    document.getElementById('pager').replaceChildren(pagination(state.page, Math.max(1, Math.ceil(state.total / state.perPage)), (p) => { state.page = p; load(); }));
  } catch (err) {
    if (err.status === 401) { location.href = '/login'; return; }
    list.replaceChildren(errorBox(err));
  }
}

document.getElementById('search-form').addEventListener('submit', (e) => {
  e.preventDefault();
  state.q = document.getElementById('q').value.trim();
  state.tag = document.getElementById('tag').value.trim();
  state.page = 1;
  load();
});

renderNav('shared');
load();
