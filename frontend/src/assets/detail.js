import { get, patch, del, post } from './api.js';
import { el, renderNav, errorBox } from './ui.js';

const id = new URLSearchParams(location.search).get('id');

async function load() {
  const box = document.getElementById('detail');
  try {
    const b = await get(`/api/v1/bookmarks/${id}`);
    box.replaceChildren(
      el('h1', {}, b.title || b.url),
      el('div', { class: 'url muted', style: 'word-break:break-all' }, b.url),
      el('div', { class: 'row', style: 'margin:10px 0' },
        ...(b.tags || []).map((t) => el('span', { class: 'tag' }, typeof t === 'string' ? t : t.name))),
      b.description ? el('p', {}, b.description) : null,
      b.notes ? el('p', { class: 'muted' }, b.notes) : null,
      el('div', { class: 'row' },
        el('a', { class: 'btn', href: b.url, target: '_blank' }, '打开原页'),
        el('button', { class: 'btn', onclick: () => save({ is_unread: !b.is_unread }) }, b.is_unread ? '标已读' : '标未读'),
        el('button', { class: 'btn', onclick: () => save({ is_archived: !b.is_archived }) }, b.is_archived ? '取消归档' : '归档'),
        el('button', { class: 'btn', onclick: () => save({ is_shared: !b.is_shared }) }, b.is_shared ? '取消共享' : '共享'),
        b.archive_url ? el('a', { class: 'btn', href: b.archive_url, target: '_blank' }, 'Archive 快照') : null,
        el('button', { class: 'btn danger', onclick: async () => { if (confirm('删除？')) { await del(`/api/v1/bookmarks/${id}`); location.href = '/bookmarks'; } } }, '删除')),
    );
    // 快照预览与资产。
    const snapResp = await fetch(`/api/v1/bookmarks/${id}/snapshot`);
    if (snapResp.ok) {
      box.append(el('h2', { style: 'margin-top:16px' }, '快照预览'));
      const iframe = el('iframe', { src: `/api/v1/bookmarks/${id}/snapshot`, style: 'width:100%;height:420px;border:1px solid var(--border);border-radius:8px;background:#fff' });
      box.append(iframe);
    }
    const assets = await get(`/api/v1/bookmarks/${id}/assets`).catch(() => null);
    if (assets) {
      document.getElementById('assets-card').style.display = 'block';
      const list = document.getElementById('assets');
      list.replaceChildren(...(assets.results || []).map((a) => el('div', { class: 'row', style: 'padding:4px 0' },
        el('a', { href: `/api/v1/bookmarks/${id}/assets/${a.id}` }, a.display_name || a.id),
        el('span', { class: 'muted' }, `${a.content_type} · ${a.file_size}B`),
        el('button', { class: 'btn sm danger', onclick: async () => { await del(`/api/v1/bookmarks/${id}/assets/${a.id}`); load(); } }, '删除'))));
    }
  } catch (err) {
    box.replaceChildren(errorBox(err));
  }
}

async function save(patchBody) {
  await patch(`/api/v1/bookmarks/${id}`, patchBody);
  load();
}

document.getElementById('upload-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const file = document.getElementById('file').files[0];
  if (!file) return;
  const form = new FormData();
  form.append('file', file);
  const resp = await fetch(`/api/v1/bookmarks/${id}/assets`, { method: 'POST', body: form, credentials: 'same-origin' });
  if (resp.ok) { load(); } else { alert(`上传失败: ${resp.status}`); }
});

renderNav('bookmarks');
load();
