// 公共 UI 辅助：导航栏、标签渲染、分页。
import { get } from './api.js';

export function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === 'class') node.className = v;
    else if (k.startsWith('on') && typeof v === 'function') node.addEventListener(k.slice(2), v);
    else if (v !== undefined && v !== null) node.setAttribute(k, v);
  }
  for (const c of children.flat()) {
    if (c === null || c === undefined) continue;
    node.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return node;
}

export async function renderNav(active) {
  const nav = el('div', { class: 'navbar' });
  nav.append(el('a', { class: 'brand', href: '/bookmarks' }, 'LinkList'));
  const links = [
    ['bookmarks', '/bookmarks', '书签'],
    ['shared', '/shared', '共享'],
    ['settings', '/settings', '设置'],
  ];
  const navEl = el('nav');
  for (const [key, href, label] of links) {
    navEl.append(el('a', { href, style: active === key ? 'font-weight:600' : '' }, label));
  }
  nav.append(navEl);
  const right = el('div', { class: 'right' });
  try {
    const me = await get('/api/v1/auth/me');
    right.append(el('span', { class: 'muted' }, me.username || me.user?.username || ''));
    const logout = el('button', { class: 'btn sm', onclick: async () => { await get('/api/v1/auth/logout').catch(() => {}); location.href = '/login'; } }, '退出');
    right.append(logout);
    // 管理员入口。
    try {
      const profile = await get('/api/v1/auth/me');
      const uname = profile.username || profile.user?.username;
      if (uname) {
        const users = await get('/api/v1/admin/users').catch(() => null);
        if (users) right.append(el('a', { href: '/admin' }, '管理'));
      }
    } catch {}
  } catch {}
  nav.append(right);
  document.body.prepend(nav);
}

export function tagChips(tags, activeTag, onSelect) {
  const wrap = el('div', { class: 'row', style: 'margin:8px 0' });
  for (const t of tags) {
    wrap.append(el('span', {
      class: `tag${activeTag === t ? ' active' : ''}`,
      onclick: () => onSelect(t),
    }, t));
  }
  return wrap;
}

export function pagination(page, totalPages, onPage) {
  if (totalPages <= 1) return el('div');
  const wrap = el('div', { class: 'pagination' });
  if (page > 1) wrap.append(el('button', { class: 'btn sm', onclick: () => onPage(page - 1) }, '上一页'));
  wrap.append(el('span', { class: 'muted', style: 'align-self:center' }, `${page} / ${totalPages}`));
  if (page < totalPages) wrap.append(el('button', { class: 'btn sm', onclick: () => onPage(page + 1) }, '下一页'));
  return wrap;
}

export function errorBox(err) {
  return el('div', { class: 'error-text' }, err.message || String(err));
}
