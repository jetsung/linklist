import { get, post, put, del, qs } from './api';
import { toastSuccess, toastError, showPendingToast } from './toast';
import { confirmDialog } from './confirm';

interface Bookmark {
  id: number; url: string; title?: string; description?: string; notes?: string; user_id?: number;
  is_unread: boolean; is_archived: boolean; is_shared: boolean; tags: (string | { name: string })[];
  created_at?: string; username?: string;
  // 共享页操作判定（对齐原版 BookmarkItem / BookmarkListContext）。
  is_editable?: boolean;
  has_snapshot?: boolean;
  archive_url?: string | null;
  // 预览图（对齐上游 bookmark.preview_image_file → /previews/{id}）。
  has_preview_image?: boolean;
}
interface ListResp { results: Bookmark[]; total: number; page: number; per_page: number; }
interface TagItem { name: string; count?: number; }

const state = {
  q: '', filter: '', bundle: null as number | null, page: 1, perPage: 30, total: 0,
  sort: 'added_desc', sharedFilter: 'off' as 'off' | 'yes' | 'no', unreadFilter: 'off' as 'off' | 'yes' | 'no',
  bulkActive: false,
  selected: new Set<number>(),
  dateDisplay: 'relative' as 'relative' | 'absolute' | 'hidden',
  // 标签云分组模式（profile.tag_grouping，对齐原版 alphabetical/disabled）。
  tagGrouping: 'disabled' as 'alphabetical' | 'disabled',
  // 描述显示模式（profile.bookmark_description_display，对齐原版 inline/separate）。
  descDisplay: 'inline' as 'inline' | 'separate',
  // 是否显示书签 URL（profile.display_url，对齐原版 show_url）。
  showUrl: true as boolean,
  // 永久显示备注（profile.permanent_notes，对齐原版 show_notes / permanent_notes）。
  permanentNotes: false as boolean,
  // 书签操作显示开关（profile.display_*_bookmark_action，对齐上游 show_*_action）。
  showViewAction: true as boolean,
  showEditAction: true as boolean,
  showArchiveAction: true as boolean,
  showRemoveAction: true as boolean,
  // 书签打开方式（profile.bookmark_link_target，对齐上游 link_target）。
  linkTarget: '_blank' as string,
  // 启用预览图片（profile.enable_preview_images，对齐上游 show_preview_images）。
  enablePreviewImages: false as boolean,
  // 启用网站图标（profile.enable_favicons，对齐上游 enable_favicons）。
  enableFavicons: true as boolean,
  sharedUser: '' as string,
  // 页面模式（shared 页仅暴露排序偏好，对齐原版 editable_fields=["sort"]）。
  mode: 'active' as 'active' | 'archived' | 'shared',
  // 用户保存的默认偏好（作为"是否被修改"的基准，对齐原版 user_profile.search_preferences）。
  savedPrefs: { sort: 'added_desc', shared: 'off', unread: 'off' } as { sort: string; shared: string; unread: string },
  // 共享页匿名视图：未登录走 /api/v1/shared/public（仅公开共享，对齐 linkding 匿名 shared）。
  sharedAnon: false,
};

const SORT_VALUES = ['added_asc', 'added_desc', 'modified_asc', 'modified_desc', 'title_asc', 'title_desc'];

function totalPages() { return Math.max(1, Math.ceil(state.total / state.perPage)); }
function host(url: string): string { try { return new URL(url).host; } catch { return ''; } }
function fmtDate(s: string | undefined, mode: 'relative' | 'absolute' | 'hidden' = 'relative'): string {
  if (!s) return '';
  const d = new Date(s);
  if (Number.isNaN(d.getTime())) return s.slice(0, 10);
  if (mode === 'absolute') {
    // 对齐 linkding humanize_absolute_date：一周内 Today/Yesterday/星期几，超过一周短日期。
    const now = new Date();
    const startOfDay = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
    const diffDays = Math.floor((startOfDay(now) - startOfDay(d)) / 86400000);
    if (diffDays <= 0) return 'Today';
    if (diffDays === 1) return 'Yesterday';
    if (diffDays < 7) return ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'][d.getDay()];
    const y = d.getFullYear();
    const m = String(d.getMonth() + 1).padStart(2, '0');
    const day = String(d.getDate()).padStart(2, '0');
    return `${y}-${m}-${day}`;
  }
  const diff = Date.now() - d.getTime();
  const day = 86400000;
  if (diff < day) return 'Today';
  if (diff < 2 * day) return 'Yesterday';
  if (diff < 7 * day) return `${Math.floor(diff / day)} days ago`;
  if (diff < 30 * day) return `${Math.floor(diff / (7 * day))} weeks ago`;
  if (diff < 365 * day) return `${Math.floor(diff / (30 * day))} months ago`;
  return `${Math.floor(diff / (365 * day))} years ago`;
}

// Wayback fallback URL：无 IA 存档记录时按书签添加时间生成兜底链接
//（对齐上游 generate_fallback_webarchive_url；无任何快照时 archive.org 会展示错误页，上游注释已声明该行为）。
function fallbackArchiveUrl(url: string, createdAt: string): string {
  const d = new Date(createdAt);
  if (isNaN(d.getTime())) return '';
  const p = (n: number) => String(n).padStart(2, '0');
  const ts = `${d.getUTCFullYear()}${p(d.getUTCMonth() + 1)}${p(d.getUTCDate())}${p(d.getUTCHours())}${p(d.getUTCMinutes())}${p(d.getUTCSeconds())}`;
  return `https://web.archive.org/web/${ts}/${url}`;
}

// ---- 列表项（对齐 bookmark_list.html DOM）----
// 预览图：渲染在内容块之后（li 为 flex，预览图固定 100×60 排右侧）。
// 对齐上游 bookmark_list.html：仅 show_preview_images（后台"启用预览图片"）开启时才渲染预览块；
// 有缓存 → img，无缓存 → placeholder。开关关闭时什么都不渲染（上游行为）。
function appendPreview(li: HTMLLIElement, b: Bookmark): void {
  if (!state.enablePreviewImages) return;
  if (b.has_preview_image) {
    const img = document.createElement('img');
    img.className = 'preview-image';
    img.loading = 'lazy';
    img.src = `/previews/${b.id}`;
    img.alt = b.title || b.url;
    li.append(img);
  } else {
    const ph = document.createElement('div');
    ph.className = 'preview-image placeholder';
    const inner = document.createElement('div'); inner.className = 'img';
    ph.append(inner);
    li.append(ph);
  }
}

function renderBookmarkItem(b: Bookmark): HTMLLIElement {
  const li = document.createElement('li');
  li.dataset.bookmarkId = String(b.id);
  if (b.is_unread) li.classList.add('unread');

  const content = document.createElement('div'); content.className = 'content';

  // title：复选框 + favicon + 链接
  const titleDiv = document.createElement('div'); titleDiv.className = 'title';
  const label = document.createElement('label'); label.className = 'form-checkbox bulk-edit-checkbox';
  const cb = document.createElement('input'); cb.type = 'checkbox'; cb.name = 'bookmark_id'; cb.value = String(b.id);
  cb.checked = state.selected.has(b.id);
  cb.addEventListener('change', () => {
    if (cb.checked) state.selected.add(b.id); else state.selected.delete(b.id);
    updateBulkBar();
  });
  const icon = document.createElement('i'); icon.className = 'form-icon';
  label.append(cb, icon);
  // favicon：受后台"启用网站图标"开关控制（对齐上游 enable_favicons）。
  let fav: HTMLImageElement | null = null;
  if (state.enableFavicons) {
    fav = document.createElement('img'); fav.className = 'favicon';
    fav.src = `/favicon/${host(b.url)}`; fav.alt = '';
    fav.addEventListener('error', () => fav?.remove());
  }
  const a = document.createElement('a'); a.href = b.url; a.target = state.linkTarget; a.rel = 'noopener';
  const span = document.createElement('span'); span.textContent = b.title || b.url; a.append(span);
  if (fav) titleDiv.append(fav);
  titleDiv.append(a);
  content.append(titleDiv);

  // url-path：show_url=false（后台"显示书签 URL"关闭）时整块不渲染（对齐上游 {% if bookmark_list.show_url %}）。
  if (state.showUrl) {
    const urlDiv = document.createElement('div'); urlDiv.className = 'url-path truncate';
    const ua = document.createElement('a'); ua.href = b.url; ua.target = state.linkTarget; ua.rel = 'noopener'; ua.className = 'url-display';
    ua.textContent = b.url; urlDiv.append(ua); content.append(urlDiv);
  }

  // description + tags：对齐上游 bookmark_list.html 两种模式。
  // inline（同行显示）：单个 .description.inline.truncate 块，标签与描述之间用 "|" 分隔；
  // separate（分块显示）：.description.separate 与 .tags 各自独立成块。
  if (state.descDisplay === 'inline' && (b.description || b.tags?.length)) {
    const inline = document.createElement('div'); inline.className = 'description inline truncate';
    if (b.tags?.length) {
      const tags = document.createElement('span'); tags.className = 'tags';
      for (const t of b.tags) {
        const name = typeof t === 'string' ? t : t.name;
        const ta = document.createElement('a'); ta.textContent = name;
        ta.href = `/bookmarks?q=${encodeURIComponent('#' + name)}`;
        tags.append(ta);
      }
      inline.append(tags);
    }
    if (b.tags?.length && b.description) inline.append('|');
    if (b.description) {
      const d = document.createElement('span'); d.textContent = b.description;
      inline.append(d);
    }
    content.append(inline);
  } else {
    if (b.description) {
      const d = document.createElement('div'); d.className = 'description separate'; d.textContent = b.description;
      content.append(d);
    }
    if (b.tags?.length) {
      const tags = document.createElement('div'); tags.className = 'tags';
      for (const t of b.tags) {
        const name = typeof t === 'string' ? t : t.name;
        const ta = document.createElement('a'); ta.textContent = name;
        ta.href = `/bookmarks?q=${encodeURIComponent('#' + name)}`;
        tags.append(ta);
      }
      content.append(tags);
    }
  }

  // actions：对齐原版 bookmark_list.html——日期(快照链接) | View [Edit Archive Remove] [extra-actions] / Shared by。
  const actions = document.createElement('div'); actions.className = 'actions';
  const isSharedPage = state.filter === 'shared';
  const editable = isSharedPage ? b.is_editable === true : true;

  // 日期：本地快照 > Wayback archive_url > fallback URL > 纯文本（对齐上游 BookmarkItem.snapshot_url 优先级）。
  // 上游所有列表页（主列表/共享/归档）日期均可能为链接，仅 Bundle 预览（is_preview）例外。
  // hidden 模式（后台"隐藏日期"）整个日期块连同分隔符都不渲染（对齐上游 display_date 为空时的模板分支）。
  if (b.created_at && state.dateDisplay !== 'hidden') {
    const dateText = fmtDate(b.created_at, state.dateDisplay);
    let snapshotHref: string;
    let snapshotTitle: string;
    if (b.has_snapshot) {
      snapshotHref = `/api/v1/bookmarks/${b.id}/snapshot`;
      snapshotTitle = 'View latest snapshot';
    } else {
      snapshotHref = b.archive_url || fallbackArchiveUrl(b.url, b.created_at);
      snapshotTitle = 'View snapshot on the Internet Archive Wayback Machine';
    }
    if (snapshotHref) {
      const da = document.createElement('a');
      da.href = snapshotHref; da.title = snapshotTitle;
      da.target = '_blank'; da.rel = 'noopener';
      da.textContent = dateText;
      actions.append(da);
    } else {
      const d = document.createElement('span'); d.textContent = dateText; actions.append(d);
    }
    const sep = document.createElement('span'); sep.textContent = '|'; actions.append(sep);
  }

  // View：共享页与自有页均展示（原版 show_view_action），打开详情弹窗。
  if (state.showViewAction) {
    const view = document.createElement('a'); view.href = '#'; view.className = 'view-action';
    view.textContent = 'View'; view.addEventListener('click', (e) => { e.preventDefault(); openDetails(b); });
    actions.append(view);
  }

  if (isSharedPage && !editable) {
    // 他人共享的书签：只读，显示归属（对齐原版 else 分支 "Shared by"）。
    const s = document.createElement('span'); s.textContent = 'Shared by ';
    const u = document.createElement('a');
    const qp = new URLSearchParams(window.location.search);
    qp.delete('user'); qp.delete('page');
    qp.set('user', b.username || '');
    u.href = `${window.location.pathname}?${qp.toString()}`;
    u.textContent = b.username || `#${b.user_id ?? ''}`;
    s.append(u);
    actions.append(s);
    content.append(actions);
    li.append(content);
    appendPreview(li, b);
    return li;
  }

  // 所有者操作：Edit / Archive / Remove（共享页仅自己的书签可见；各项受"书签操作"开关控制）。
  if (state.showEditAction) {
    const edit = document.createElement('a');
    edit.href = `/bookmarks/edit?id=${b.id}${isSharedPage ? '&return_url=/bookmarks/shared' : ''}`;
    edit.textContent = 'Edit'; actions.append(edit);
  }

  if (state.showArchiveAction) {
    const arch = document.createElement('button'); arch.type = 'button';
    arch.className = 'btn btn-link btn-sm';
    arch.textContent = b.is_archived ? 'Unarchive' : 'Archive';
    arch.addEventListener('click', () => archiveBookmark(b, !b.is_archived));
    actions.append(arch);
  }

  // Remove 紧跟 Archive/Unarchive 之后；extra-actions(Unread/Shared) 在最后，带 | 分隔（AGENTS.md 规定顺序）。
  if (state.showRemoveAction) {
    const rm = document.createElement('button'); rm.type = 'button';
    rm.className = 'btn btn-link btn-sm'; rm.textContent = 'Remove';
    rm.addEventListener('click', () => removeBookmark(b));
    actions.append(rm);
  }

  const extra = document.createElement('div'); extra.className = 'extra-actions';
  const mkIcon = (label: string, iconName: string, fn: () => void, confirmQ?: string) => {
    const btn = document.createElement('button'); btn.type = 'button';
    btn.className = 'btn btn-link btn-sm btn-icon';
    btn.innerHTML = `<svg width="16" height="16"><use href="/icons.svg#${iconName}"></use></svg>${label}`;
    btn.addEventListener('click', async () => {
      if (!confirmQ || await confirmDialog(confirmQ)) fn();
    });
    return btn;
  };
  if (b.is_unread) {
    extra.append(mkIcon('Unread', 'unread', () => toggleUnread(b), '标记为已读？'));
  }
  if (b.is_shared) {
    extra.append(mkIcon('Shared', 'share', () => toggleShared(b), 'Unshare?'));
  }
  if (b.notes && !state.permanentNotes) {
    const notesBtn = document.createElement('button'); notesBtn.type = 'button';
    notesBtn.className = 'btn btn-link btn-sm btn-icon toggle-notes';
    notesBtn.innerHTML = `<svg width="16" height="16"><use href="/icons.svg#note"></use></svg>Notes`;
    notesBtn.addEventListener('click', (e) => {
      e.preventDefault(); e.stopPropagation();
      (e.currentTarget as HTMLElement).closest('li')?.classList.toggle('show-notes');
    });
    extra.append(notesBtn);
  }
  if (extra.childElementCount > 0) {
    const xsep = document.createElement('span'); xsep.textContent = '|';
    actions.append(xsep, extra);
  }
  // notes：有备注时渲染备注块（列表 CSS 默认隐藏，permanent_notes 或 li.show-notes 时显示）。
  // 非 permanent_notes 时在 extra-actions 提供 Notes 切换按钮（对齐上游 toggle-notes）。
  if (b.notes) {
    const notes = document.createElement('div'); notes.className = 'notes';
    const md = document.createElement('div'); md.className = 'markdown'; md.textContent = b.notes;
    notes.append(md);
    content.append(notes);
  }
  content.append(actions);
  li.append(content);
  appendPreview(li, b);
  return li;
}


async function archiveBookmark(b: Bookmark, archived: boolean) {
  await post(`/api/v1/bookmarks/${b.id}/${archived ? 'archive' : 'unarchive'}`);
  toastSuccess(archived ? '书签已归档' : '书签已取消归档', { blocking: true });
  await load();
}

async function toggleUnread(b: Bookmark) {
  await fetch(`/api/v1/bookmarks/${b.id}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin', body: JSON.stringify({ is_unread: !b.is_unread }),
  });
  toastSuccess(b.is_unread ? '已标记为已读' : '已标记为未读', { blocking: true });
  await load();
}

async function toggleShared(b: Bookmark) {
  await fetch(`/api/v1/bookmarks/${b.id}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin', body: JSON.stringify({ is_shared: !b.is_shared }),
  });
  toastSuccess(b.is_shared ? '已取消共享' : '已共享', { blocking: true });
  await load();
}

async function removeBookmark(b: Bookmark) {
  if (!(await confirmDialog(`删除书签"${b.title || b.url}"？`))) return;
  await del(`/api/v1/bookmarks/${b.id}`);
  toastSuccess('书签已删除', { blocking: true });
  state.selected.delete(b.id);
  await load();
}
let lastItems: Bookmark[] = [];

function renderList(items: Bookmark[]) {
  const container = document.getElementById('bookmark-list-container');
  let ul = document.getElementById('bookmark-list') as HTMLUListElement | null;
  // 空状态会把容器内容替换为 empty 块（ul 被移除）；再次有数据时需重建 ul，
  // 否则后续筛选恢复后列表永远不渲染（对齐上游服务端模板重渲染行为）。
  if (!ul && container) {
    ul = document.createElement('ul');
    ul.id = 'bookmark-list';
    ul.className = 'bookmark-list';
    ul.setAttribute('role', 'list');
    container.replaceChildren(ul);
  }
  if (!ul) return;
  if (!items.length) {
    container?.replaceChildren(emptyBlock());
    return;
  }
  ul.replaceChildren(...items.map(renderBookmarkItem));
}

function emptyBlock(): HTMLElement {
  const div = document.createElement('div'); div.className = 'empty mt-4';
  div.innerHTML = `<p class="empty-title h5">You have no bookmarks yet</p>
    <p class="empty-subtitle">
      You can get started by <a href="/bookmarks/new">adding</a> bookmarks,
      <a href="/settings/general">importing</a> your existing bookmarks or configuring the
      <a href="/settings/integrations">browser extension</a> or the <a href="/settings/integrations">bookmarklet</a>.
    </p>`;
  return div;
}

// ---- 分页（逐字对齐 linkding templatetags/pagination.py 的可见页算法）----
const NUM_ADJACENT_PAGES = 2;

/// 生成应渲染的页码列表；空洞以 -1 表示（渲染为省略号）。
/// 移植自 linkding `get_visible_page_numbers`。
function visiblePageNumbers(current: number, numPages: number): number[] {
  const pages = new Set<number>();
  for (let p = Math.max(1, current - NUM_ADJACENT_PAGES); p <= Math.min(numPages, current + NUM_ADJACENT_PAGES); p++) {
    pages.add(p);
  }
  pages.add(1);
  pages.add(numPages);
  const sorted = [...pages].sort((a, b) => a - b);
  const out: number[] = [];
  for (const p of sorted) {
    if (out.length > 0 && out[out.length - 1] < p - 1) out.push(-1);
    out.push(p);
  }
  return out;
}

function renderPagination() {
  const wrap = document.querySelector('.bookmark-pagination .pagination') as HTMLUListElement | null;
  if (!wrap) return;
  wrap.replaceChildren();
  const tp = totalPages();
  const li = (cls: string, html: string) => {
    const el = document.createElement('li'); el.className = cls; el.innerHTML = html; return el;
  };
  const link = (p: number, text: string) =>
    `<a href="${pageUrl(p)}" data-page="${p}">${text}</a>`;
  wrap.append(state.page > 1
    ? li('page-item', link(state.page - 1, 'Previous'))
    : li('page-item disabled', '<a href="#" tabindex="-1">Previous</a>'));
  for (const p of visiblePageNumbers(state.page, tp)) {
    wrap.append(p === -1
      ? li('page-item', '<span>...</span>')
      : p === state.page
        ? li('page-item active', link(p, String(p)))
        : li('page-item', link(p, String(p))));
  }
  wrap.append(state.page < tp
    ? li('page-item', link(state.page + 1, 'Next'))
    : li('page-item disabled', '<a href="#" tabindex="-1">Next</a>'));
  wrap.querySelectorAll('a[data-page]').forEach((el) => {
    el.addEventListener('click', (ev) => {
      ev.preventDefault();
      goToPage(Number((el as HTMLElement).dataset.page));
    });
  });
}

/// 翻页：同步 URL query（?page=N）并重新加载；支持浏览器前进/后退。
function goToPage(p: number) {
  const tp = totalPages();
  const page = Math.min(Math.max(1, p), tp);
  if (page === state.page && new URL(window.location.href).searchParams.get('page') === String(page)) return;
  state.page = page;
  const u = new URL(window.location.href);
  if (page > 1) u.searchParams.set('page', String(page)); else u.searchParams.delete('page');
  window.history.pushState({}, '', u.pathname + u.search);
  load();
  window.scrollTo({ top: 0 });
 }

function pageUrl(p: number): string {
  const u = new URL(window.location.href);
  u.searchParams.set('page', String(p));
  return u.pathname + u.search;
}

// ---- 标签云（对齐 tag_cloud.html：selected 红字 + unselected 按字母分组首字高亮）----
function isUntaggedQuery() {
  const q = (state.q || '').trim().toLowerCase();
  return q === '!untagged' || q === 'untagged';
}

function syncTagsPanelVisibility() {
  // 无标签搜索：隐藏右侧 TAGS 侧栏（该查询下标签云无意义）。
  const section = document.getElementById('tag-cloud')?.closest('section');
  if (section) (section as HTMLElement).style.display = isUntaggedQuery() ? 'none' : '';
}


// 提取查询中的全部 #标签名（对齐 linkding extract_tag_names_from_query 的 TagExpression 提取）。
function extractSelectedTagNames(query: string): string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  for (const part of query.split(/\s+/)) {
    if (!part.startsWith('#') || part.length < 2) continue;
    const name = part.slice(1);
    const key = name.toLowerCase();
    if (!seen.has(key)) { seen.add(key); out.push(name); }
  }
  return out;
}
function renderTagCloud() {
  const box = document.getElementById('tag-cloud');
  if (!box) return;
  // 对齐 linkding TagCloudContext：selected 区 = 查询中的全部 #标签（逐个带移除链接）；
  // unselected 区 = 当前过滤结果统计出的标签去掉已选中的，链接为"现有 q 追加 #标签"（AND 组合）。
  const selectedNames = extractSelectedTagNames(state.q || '');
  const removeTagFromQuery = (query: string, name: string) => {
    const parts = query.split(/\s+/).filter(Boolean);
    const kept = parts.filter((p) => p.toLowerCase() !== `#${name.toLowerCase()}`);
    return kept.join(' ');
  };
  const addTagToQuery = (query: string, name: string) => {
    const parts = query.split(/\s+/).filter(Boolean);
    return [...parts, `#${name}`].join(' ');
  };
  const buildHref = (q: string) => {
    // 保留当前页面固有的栏目参数（未读 unread、共享 shared、bundle 等），
    // 仅替换 q，使多标签 AND 组合在原视图上继续筛选。
    const qp = new URLSearchParams(window.location.search);
    qp.delete('page');
    if (q) qp.set('q', q); else qp.delete('q');
    return `${window.location.pathname}${qp.toString() ? `?${qp.toString()}` : ''}`;
  };
  const sel = box.querySelector('.selected-tags') as HTMLElement;
  sel.replaceChildren();
  for (const name of selectedNames) {
    // 对齐 linkding RemoveTagItem：移除该标签，保留查询中的其他条件。
    const a = document.createElement('a');
    a.className = 'text-bold mr-2';
    a.href = buildHref(removeTagFromQuery(state.q || '', name));
    a.innerHTML = `<span>-${name}</span>`;
    sel.append(a);
  }
  const un = box.querySelector('.unselected-tags') as HTMLElement;
  un.replaceChildren();
  const selectedLower = new Set(selectedNames.map((n) => n.toLowerCase()));
  const tags = state.tags.filter((t) => t.name && !selectedLower.has(t.name.toLowerCase()));
  if (tags.length) {
    // 对齐 linkding TagGroup：alphabetical 模式按首字符分组（大小写不敏感），
    // CJK 标签归入末组 "Ideographic"；disabled 模式单组不高亮。
    const cjk = /[\u4e00-\u9fff]/;
    const groups = new Map<string, typeof tags>();
    if (state.tagGrouping === 'alphabetical') {
      for (const t of tags) {
        const ch = t.name[0].toLowerCase();
        const key = cjk.test(ch) ? 'Ideographic' : ch;
        if (!groups.has(key)) groups.set(key, []);
        groups.get(key)!.push(t);
      }
    } else {
      groups.set('', tags);
    }
    for (const [, groupTags] of groups) {
      const p = document.createElement('p'); p.className = 'group';
      // 对齐上游 tag_cloud.html：分组只通过"每组首标签高亮首字符"体现，不另渲染字母标题。
      for (const t of groupTags) {
        const a = document.createElement('a');
        a.dataset.isTagItem = '';
        // 对齐 linkding AddTagItem：在当前查询后追加 #标签（多个标签 AND 组合进一步筛选）。
        a.href = buildHref(addTagToQuery(state.q || '', t.name));
        if (state.tagGrouping === 'alphabetical') {
          // 每组首标签高亮首字符（对齐 tag_cloud.html highlight-char）。
          const first = t === groupTags[0];
          a.innerHTML = first
            ? `<span class="highlight-char">${t.name[0]}</span><span>${t.name.slice(1)}</span>`
            : `<span>${t.name}</span>`;
        } else {
          a.innerHTML = `<span>${t.name}</span>`;
        }
        p.append(a);
      }
      un.append(p);
    }
  }
  syncTagsPanelVisibility();
}

// 标签选择已改为真实链接（?q=#标签），不再使用本地状态切换。
// ---- 批量编辑（对齐 bulk_edit_bar.html + ld-bookmark-page.active 行为）----
function updateBulkBar() {
  const page = document.querySelector('ld-bookmark-page') as HTMLElement | null;
  const c = document.getElementById('bulk-count');
  const bar = document.getElementById('bulk-edit-bar');
  if (c) c.textContent = String(state.selected.size);
  if (!bar) return;
  if (state.bulkActive) {
    page?.classList.add('active');
    bar.dataset.count = String(state.selected.size);
  }
}

function setBulkActive(on: boolean) {
  state.bulkActive = on;
  const page = document.querySelector('ld-bookmark-page') as HTMLElement | null;
  page?.classList.toggle('active', on);
  // 开关按钮常驻：点击在开/关之间切换，不隐藏。
  const toggle = document.getElementById('bulk-edit-toggle') as HTMLElement | null;
  if (toggle) toggle.classList.toggle('active', on);
  if (!on) {
    state.selected.clear();
    document.querySelectorAll('#bookmark-list input[type=checkbox]').forEach((cb) => {
      (cb as HTMLInputElement).checked = false;
    });
  }
  updateBulkBar();
}

// ---- 详情弹窗（对齐 details/modal.html + form.html）----
async function openDetails(b: Bookmark) {
  const overlay = document.getElementById('details-overlay');
  if (!overlay) return;
  overlay.classList.add('active');
  (overlay.querySelector('.modal-title') as HTMLElement).textContent = b.title || b.url;

  const body = overlay.querySelector('.modal-body') as HTMLElement;
  body.replaceChildren();
  const form = document.createElement('form'); form.className = 'details-form';

  // weblinks：URL 行
  const weblinks = document.createElement('div'); weblinks.className = 'weblinks';
  const wl = document.createElement('a'); wl.className = 'weblink'; wl.href = b.url; wl.target = '_blank'; wl.rel = 'noopener';
  wl.innerHTML = state.enableFavicons
    ? `<img class="favicon" src="/favicon/${host(b.url)}" alt=""><span>${b.url}</span>`
    : `<span>${b.url}</span>`;
  weblinks.append(wl);
  form.append(weblinks);

  const sections = document.createElement('div'); sections.className = 'sections grid columns-2 columns-sm-1 gap-0';
  const sec = (cls: string, title: string, node: HTMLElement) => {
    const s = document.createElement('section'); s.className = cls;
    const h = document.createElement('h3'); h.textContent = title;
    s.append(h, node); sections.append(s);
  };

  // Status：Archived / Unread / Shared 复选框（可保存）
  const status = document.createElement('div'); status.className = 'd-flex'; status.style.gap = '.8rem';
  const mkCheck = (label: string, checked: boolean, fn: () => void) => {
    const wrap = document.createElement('label'); wrap.className = 'form-checkbox';
    const input = document.createElement('input'); input.type = 'checkbox'; input.checked = checked;
    input.addEventListener('change', fn);
    const i = document.createElement('i'); i.className = 'form-icon';
    wrap.append(input, i, document.createTextNode(label));
    return wrap;
  };
  status.append(
    mkCheck('Archived', b.is_archived, () => archiveBookmark(b, !b.is_archived)),
    mkCheck('Unread', b.is_unread, () => toggleUnread(b)),
    mkCheck('Shared', b.is_shared, () => toggleShared(b)),
  );
  sec('status col-2', 'Status', status);

  // Tags
  if (b.tags?.length) {
    const div = document.createElement('div');
    for (const t of b.tags) {
      const name = typeof t === 'string' ? t : t.name;
      const a = document.createElement('a');
      a.href = `/bookmarks?q=${encodeURIComponent('#' + name)}`; a.textContent = name;
      a.className = 'mr-2';
      div.append(a);
    }
    sec('tags col-1', 'Tags', div);
  }

  // Date added
  const dateDiv = document.createElement('div');
  dateDiv.innerHTML = `<span>${b.created_at ? new Date(b.created_at).toLocaleString() : ''}</span>`;
  sec('date-added col-1', 'Date added', dateDiv);

  // Description
  if (b.description) {
    const d = document.createElement('div'); d.textContent = b.description;
    sec('description col-2', 'Description', d);
  }
  // Notes
  if (b.notes) {
    const d = document.createElement('div'); d.className = 'markdown'; d.textContent = b.notes;
    sec('notes col-2', 'Notes', d);
  }
  form.append(sections);
  body.append(form);

  const editBtn = overlay.querySelector('.modal-edit') as HTMLAnchorElement;
  editBtn.href = `/bookmarks/edit?id=${b.id}`;
  const delBtn = overlay.querySelector('.modal-delete') as HTMLButtonElement;
  const clone = delBtn.cloneNode(true) as HTMLButtonElement;
  delBtn.replaceWith(clone);
  clone.addEventListener('click', async () => {
    if (!(await confirmDialog('确定删除该书签？'))) return;
    await del(`/api/v1/bookmarks/${b.id}`);
    overlay.classList.remove('active');
    await load();
  });
}

// ---- 数据加载 ----
async function load() {
  // per_page 不显式传：由后端按 profile.items_per_page（"每页条数"）决定（对齐上游分页器）。
  const params: Record<string, unknown> = { page: state.page, sort: state.sort };
  if (state.q) params.q = state.q;
  if (state.bundle) params.bundle = state.bundle;
  if (state.filter === 'archived') params.filter = 'archived';
  else if (state.unreadFilter === 'yes') params.filter = 'unread';
  else if (state.unreadFilter === 'no') params.filter = 'all';
  if (state.sharedFilter === 'yes') params.shared = 'yes';
  else if (state.sharedFilter === 'no') params.shared = 'no';
  if (state.unreadFilter === 'yes') params.unread = 'yes';
  else if (state.unreadFilter === 'no') params.unread = 'no';
  // 登录态共享页走 /api/v1/shared（含用户筛选）；公开视图走专用公开接口（匿名可访问）。
  const sharedParams: Record<string, unknown> = { page: params.page, sort: state.sort };
  // q 整串交给后端解析（#多标签 AND / 关键词）。
  if (state.q) sharedParams.q = state.q;
  if (state.sharedUser) sharedParams.user = state.sharedUser;
  const endpoint = state.filter === 'shared'
    ? (state.sharedAnon ? `/api/v1/shared/public${qs(sharedParams)}` : `/api/v1/shared${qs(sharedParams)}`)
    : `/api/v1/bookmarks${qs(params)}`;
  const data: ListResp = await get(endpoint);
  state.total = data.total;
  state.page = data.page;
  state.perPage = data.per_page;
  lastItems = data.results;
  renderList(lastItems);
  renderPagination();
  const totalEl = document.getElementById('bookmarks-total');
  if (totalEl) totalEl.textContent = `(${data.total})`;
}

// ---- Bundles（保存的过滤器，侧栏）----
interface BundleItem { id: number; name: string; }

// ---- 侧栏偏好（对齐上游 collapse_side_panel / hide_bundles）----
// applySidePanelPrefs：按 profile 设置把侧栏折叠为主区通栏、移除组合区块，
// 并在页头放"筛选"按钮，点击打开右侧抽屉展示侧栏内容（对齐 linkding
// ld-filter-drawer-trigger / ld-filter-drawer，本项目无 lit，直接 DOM 实现）。
function applySidePanelPrefs(collapseSidePanel: boolean, hideBundles: boolean) {
  const page = document.querySelector('ld-bookmark-page.bookmarks-page') as HTMLElement | null;
  const panel = document.querySelector('.side-panel') as HTMLElement | null;
  if (page && collapseSidePanel) page.classList.add('collapse-side-panel');
  if (hideBundles) {
    // 组合区块（bookmarks/archived 页首个 section；shared 页无组合 section）。
    const bundlesSection = document.querySelector('section[aria-labelledby$="-bundles-heading"]')
      || document.querySelector('section[aria-labelledby="bundles-heading"]');
    bundlesSection?.remove();
    // 抽屉内同步移除，避免恢复显示空组合。
    document.querySelectorAll('.filter-drawer-body section[aria-labelledby$="-bundles-heading"]')
      .forEach((el) => el.remove());
  }
  if (!page || !panel || !collapseSidePanel) return;

  // 折叠时主区占满；在页头控件区放"筛选"按钮（对齐上游 Filters 按钮）。
  const controls = page.querySelector('.header-controls');
  if (controls && !controls.querySelector('.filter-drawer-trigger')) {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'btn ml-2 filter-drawer-trigger';
    btn.textContent = '筛选';
    btn.addEventListener('click', () => openFilterDrawer(panel));
    controls.append(btn);
  }
}

function openFilterDrawer(panel: HTMLElement) {
  // 再次点击"筛选"按钮：若抽屉已打开则关闭（切换状态）。
  const existing = document.getElementById('filter-drawer') as HTMLElement | null;
  if (existing?.classList.contains('active')) {
    existing.classList.remove('active');
    const body = existing.querySelector('.filter-drawer-body') as HTMLElement;
    if (panel && body.children.length) panel.append(...body.children);
    return;
  }
  // 复用页面上已有的抽屉（重复打开不再teleport）。
  let drawer = existing;
  if (!drawer) {
    drawer = document.createElement('div');
    drawer.id = 'filter-drawer';
    drawer.className = 'modal drawer';
    drawer.setAttribute('role', 'dialog');
    drawer.setAttribute('aria-modal', 'true');
    drawer.innerHTML = `
      <div class="modal-overlay" data-close-drawer></div>
      <div class="modal-container" role="dialog" aria-modal="true">
        <div class="modal-header">
          <h2>筛选</h2>
          <button type="button" class="btn btn-noborder close" aria-label="关闭" data-close-drawer>×</button>
        </div>
        <div class="modal-body filter-drawer-body"></div>
      </div>`;
    document.body.append(drawer);
    const close = () => {
      drawer!.classList.remove('active');
      // 关闭时把筛选内容搬回侧栏（对齐上游 teleportBack）。
      const body = drawer!.querySelector('.filter-drawer-body') as HTMLElement;
      const panel = document.querySelector('.side-panel') as HTMLElement | null;
      if (panel && body.children.length) panel.append(...body.children);
    };
    drawer.querySelectorAll('[data-close-drawer]').forEach((el) => el.addEventListener('click', close));
    drawer.addEventListener('keydown', (e) => { if ((e as KeyboardEvent).key === 'Escape') close(); });
  }
  // 把侧栏内容搬进抽屉（对齐上游 teleport）。
  const body = drawer.querySelector('.filter-drawer-body') as HTMLElement;
  body.append(...panel.children);
  drawer.classList.add('active');
  (drawer.querySelector('.modal-container') as HTMLElement | null)?.focus();
}

async function loadBundles() {
  const menu = document.getElementById('bundle-menu');
  if (!menu) return;
  const data = await get('/api/v1/bundles');
  const items = (data.results || []) as BundleItem[];
  menu.textContent = '';
  const base = new URL('/bookmarks', window.location.origin);
  // 对齐 linkding：菜单直接列出各 bundle（?bundle=N）。
  for (const b of items) {
    const li = document.createElement('li');
    li.className = 'bundle-menu-item' + (state.bundle === b.id ? ' selected' : '');
    const a = document.createElement('a');
    const u = new URL(base);
    u.searchParams.set('bundle', String(b.id));
    a.href = u.pathname + u.search;
    a.textContent = b.name;
    li.append(a);
    menu.append(li);
  }
}

async function loadTags() {
  // 对齐 linkding：标签云基于当前搜索过滤后的书签统计（?q= #标签 / bundle / filter 均生效）。
  // 共享页用专用统计接口（所有用户共享书签，可按 user 筛选）。
  if (state.filter === 'shared') {
    const sp: Record<string, unknown> = {};
    // q 整串交给后端解析（与列表接口一致）。
    if (state.q) sp.q = state.q;
    if (state.sharedUser) sp.user = state.sharedUser;
    const sd = await get(`${state.sharedAnon ? '/api/v1/shared/public' : '/api/v1/shared'}/tags${qs(sp)}`);
    state.tags = ((sd.results || []) as Array<{ name: string; count?: number }>).map((t) => ({ name: t.name, count: t.count }));
    renderTagCloud();
    return;
  }
  const params: Record<string, unknown> = {};
  if (state.bundle) params.bundle = state.bundle;
  if (state.filter === 'archived') params.filter = 'archived';
  else if (state.unreadFilter === 'yes') params.filter = 'unread';
  else if (state.unreadFilter === 'no') params.filter = 'all';
  if (state.sharedFilter === 'yes') params.shared = 'yes';
  else if (state.sharedFilter === 'no') params.shared = 'no';
  if (state.unreadFilter === 'yes') params.unread = 'yes';
  else if (state.unreadFilter === 'no') params.unread = 'no';
  if (state.q) params.q = state.q;
  const data = await get(`/api/v1/bookmarks/tags${qs(params)}`);
  const items = (data.results || []) as unknown[];
  state.tags = items.map((t) => {
    if (typeof t === 'string') return { name: t };
    if (t && typeof t === 'object' && 'name' in t) {
      const name = t.name;
      const count = 'count' in t ? t.count : undefined;
      return { name: typeof name === 'string' ? name : '', count: typeof count === 'number' ? count : undefined };
    }
    return { name: '' };
  });
  renderTagCloud();
}

// ---- 批量操作 ----
async function runBulkAction(action: string, tagString?: string) {
  const ids = [...state.selected];
  if (!ids.length) { toastError('未选择任何书签', { blocking: true }); return; }
  if (action === 'bulk_delete' && !(await confirmDialog(`确定删除选中的 ${ids.length} 个书签？`))) return;
  const body: Record<string, unknown> = { ids };
  if (tagString) body.tags = tagString.split(/[\s,]+/).filter(Boolean);
  try {
    switch (action) {
      case 'bulk_archive': await post('/api/v1/bookmarks/bulk/archive', body); break;
      case 'bulk_unarchive': await post('/api/v1/bookmarks/bulk/unarchive', body); break;
      case 'bulk_delete': await post('/api/v1/bookmarks/bulk/delete', body); break;
      case 'bulk_tag': await post('/api/v1/bookmarks/bulk/tags/add', body); break;
      case 'bulk_untag': await post('/api/v1/bookmarks/bulk/tags/remove', body); break;
      case 'bulk_read': await post('/api/v1/bookmarks/bulk/mark-read', body); break;
      case 'bulk_unread': await post('/api/v1/bookmarks/bulk/mark-unread', body); break;
      case 'bulk_share': await post('/api/v1/bookmarks/bulk/share', body); break;
      case 'bulk_unshare': await post('/api/v1/bookmarks/bulk/unshare', body); break;
      case 'bulk_refresh': await post('/api/v1/bookmarks/bulk/refresh', body); break;
      default: toastError('不支持的批量操作', { blocking: true }); return;
    }
  } catch (e: unknown) {
    toastError(e instanceof Error ? e.message : '批量操作失败', { blocking: true });
    return;
  }
  toastSuccess(`已更新 ${ids.length} 个书签`, { blocking: true });
  state.selected.clear();
  setBulkActive(false);
  await Promise.all([load(), loadTags()]);
}

// ---- 初始化 ----
export function initBookmarksPage(mode: 'active' | 'archived' | 'shared') {
  state.mode = mode;
  const url = new URL(window.location.href);
  state.q = url.searchParams.get('q') || '';
  state.sharedUser = url.searchParams.get('user') || '';
  const bundleParam = url.searchParams.get('bundle');
  state.bundle = bundleParam ? Number(bundleParam) || null : null;
  // URL 参数优先；否则用已保存偏好（对齐原版 from_request(request, GET, search_preferences)）。
  const sortParam = url.searchParams.get('sort') || '';
  const sp = url.searchParams.get('shared') || '';
  const up = url.searchParams.get('unread') || '';
  const applyPrefs = (prefs: { sort: string; shared: string; unread: string }) => {
    if (SORT_VALUES.includes(prefs.sort)) state.sort = prefs.sort;
    if (prefs.shared === 'yes' || prefs.shared === 'no') state.sharedFilter = prefs.shared; else state.sharedFilter = 'off';
    if (prefs.unread === 'yes' || prefs.unread === 'no') state.unreadFilter = prefs.unread; else state.unreadFilter = 'off';
  };
  // syncPrefControls 在下方定义；先留引用，供异步偏好加载后回填。
  let syncPrefControlsRef: (() => void) | null = null;
  applyPrefs(state.savedPrefs);
  // 异步取已保存偏好，之后若 URL 没带参数则应用之（首屏先按默认渲染，避免阻塞）。
  // 共享页匿名（本人偏好 401）时读"生效资料"里访客资料保存的搜索偏好（对齐上游 search_preferences）。
  const loadSearchPrefs = () => get('/api/v1/profile/search-preferences').catch(() => {
    if (mode !== 'shared') throw new Error('skip');
    return get('/api/v1/shared/viewer-profile').then((vp: Record<string, unknown>) => {
      const sp = (vp.search_preferences ?? {}) as Record<string, unknown>;
      return { sort: sp.sort, shared: sp.shared, unread: sp.unread };
    });
  });
  loadSearchPrefs().then((d) => {
    state.savedPrefs = {
      sort: typeof d.sort === 'string' ? d.sort : 'added_desc',
      shared: d.shared === 'yes' || d.shared === 'no' ? d.shared : 'off',
      unread: d.unread === 'yes' || d.unread === 'no' ? d.unread : 'off',
    };
    if (!sortParam && !sp && !up) {
      applyPrefs(state.savedPrefs);
      state.page = 1;
      syncPrefControlsRef?.();
      load().then(loadTags).catch(() => {});
    }
  }).catch(() => {});
  if (SORT_VALUES.includes(sortParam)) state.sort = sortParam;
  if (sp === 'yes' || sp === 'no') state.sharedFilter = sp;
  if (up === 'yes' || up === 'no') state.unreadFilter = up;
  if (mode === 'archived') state.filter = 'archived';
  if (mode === 'shared') state.filter = 'shared';
  // 共享页：加载共享用户列表并填充下拉（对齐原版 User 区块）。
  if (mode === 'shared') {
    (async () => {
      try {
        const d = await get('/api/v1/shared/users');
        const users = (d.results || []) as Array<{ id: number; username: string; count?: number }>;
        const sel = document.getElementById('shared-user-select') as HTMLSelectElement | null;
        if (sel) {
          sel.replaceChildren();
          const all = document.createElement('option'); all.value = ''; all.textContent = `全部用户 (${users.reduce((s, x) => s + (x.count || 0), 0)})`;
          sel.append(all);
          for (const u of users) {
            const o = document.createElement('option'); o.value = u.username; o.textContent = `${u.username} (${u.count ?? ''})`;
            sel.append(o);
          }
          sel.value = state.sharedUser;
          sel.addEventListener('change', () => {
            state.sharedUser = sel.value;
            state.page = 1;
            const nu = new URL(window.location.href);
            if (state.sharedUser) nu.searchParams.set('user', state.sharedUser); else nu.searchParams.delete('user');
            nu.searchParams.delete('page');
            window.history.replaceState({}, '', nu);
            load().then(loadTags).catch(() => {});
          });
        }
      } catch { /* 用户列表失败不影响列表 */ }
    })();
  }

  const qInput = document.getElementById('search-input') as HTMLInputElement | null;
  if (qInput) {
    qInput.value = state.q;
    qInput.closest('form')?.addEventListener('submit', (e) => {
      e.preventDefault();
      state.q = qInput.value.trim();
      state.page = 1;
      const u = new URL(window.location.href);
      if (state.q) u.searchParams.set('q', state.q); else u.searchParams.delete('q');
      u.searchParams.delete('page');
      window.history.replaceState({}, '', u);
      load().then(loadTags).catch(() => {});
    });
  }

  // 搜索偏好（排序/共享/未读）表单：初始回填 + Apply/Save as default（对齐原版 preferences 下拉）。
  const isSharedMode = state.mode === 'shared';
  const readControls = () => {
    const sortValue = (document.getElementById('sort') as HTMLSelectElement | null)?.value;
    const sort = sortValue && SORT_VALUES.includes(sortValue) ? sortValue : 'added_desc';
    if (isSharedMode) return { sort, shared: state.sharedFilter, unread: state.unreadFilter };
    const sharedValue = document.querySelector<HTMLInputElement>('input[name=shared-filter]:checked')?.value;
    const unreadValue = document.querySelector<HTMLInputElement>('input[name=unread-filter]:checked')?.value;
    return {
      sort,
      shared: sharedValue === 'yes' || sharedValue === 'no' ? sharedValue : 'off',
      unread: unreadValue === 'yes' || unreadValue === 'no' ? unreadValue : 'off',
    };
  };
  // 清除 URL 中的偏好参数（sort/shared/unread/page）。
  // 应用=仅本次会话生效，刷新页面后按已存默认恢复；保存为默认=默认已更新，参数与默认一致无需保留。
  const clearPrefsUrlParams = () => {
    const u = new URL(window.location.href);
    u.searchParams.delete('sort');
    u.searchParams.delete('shared');
    u.searchParams.delete('unread');
    u.searchParams.delete('page');
    window.history.replaceState({}, '', u);
  };
  const syncGearBadge = () => {
    const gear = document.querySelector('.search-options .dropdown-toggle');
    const modified = state.sort !== state.savedPrefs.sort
      || state.sharedFilter !== state.savedPrefs.shared
      || state.unreadFilter !== state.savedPrefs.unread;
    if (gear) gear.classList.toggle('badge', modified);
  };

  // 搜索框右侧"过滤"按钮：点击在开/关之间切换（菜单显示纯靠 .dropdown:focus-within
  // CSS，按钮点击后焦点仍在 dropdown 内所以保持展开；再点一次需主动移除 active 并
  // blur 才能收起，同时兼容键盘/触屏）。
  const closePrefsDropdown = () => {
    document.querySelector('.search-options')?.classList.remove('active');
    (document.querySelector('.search-options .dropdown-toggle') as HTMLElement | null)?.blur();
  };
  const prefsToggle = document.querySelector('.search-options .dropdown-toggle') as HTMLElement | null;
  if (prefsToggle) {
    prefsToggle.addEventListener('click', (e) => {
      e.stopPropagation();
      const dd = document.querySelector('.search-options') as HTMLElement | null;
      if (!dd) return;
      const nowActive = dd.classList.toggle('active');
      if (!nowActive) prefsToggle.blur();
    });
    // 点击页面其他位置时收起菜单（并同步移除 active，避免下次点击误判为"开着"）。
    document.addEventListener('click', (e) => {
      const dd = document.querySelector('.search-options');
      if (dd?.classList.contains('active') && !dd.contains(e.target as Node)) {
        closePrefsDropdown();
      }
    });
  }
  // 控件回填（shared 页没有 shared/unread 控件，自动跳过）。
  const syncPrefControls = () => {
    const sel = document.getElementById('sort') as HTMLSelectElement | null;
    if (sel) sel.value = state.sort;
    const sharedInput = document.querySelector<HTMLInputElement>(`input[name=shared-filter][value=${state.sharedFilter}]`);
    const unreadInput = document.querySelector<HTMLInputElement>(`input[name=unread-filter][value=${state.unreadFilter}]`);
    if (sharedInput) {
      document.querySelectorAll<HTMLInputElement>('input[name=shared-filter]').forEach((r) => { r.checked = false; });
      sharedInput.checked = true;
    }
    if (unreadInput) {
      document.querySelectorAll<HTMLInputElement>('input[name=unread-filter]').forEach((r) => { r.checked = false; });
      unreadInput.checked = true;
    }
    syncGearBadge();
  };
  syncPrefControls();
  syncPrefControlsRef = syncPrefControls;
  document.getElementById('apply-prefs')?.addEventListener('click', () => {
    const prefs = readControls();
    state.sort = prefs.sort; state.sharedFilter = prefs.shared; state.unreadFilter = prefs.unread;
    clearPrefsUrlParams();
    syncGearBadge();
    state.page = 1;
    load().then(loadTags).catch(() => {});
    closePrefsDropdown();
  });
  // Save as default：写入用户偏好（对齐原版 search_action "save" 分支）。
  document.getElementById('save-prefs')?.addEventListener('click', () => {
    const prefs = readControls();
    put('/api/v1/profile/search-preferences', prefs).then((saved) => {
      // 保存为默认后，当前页立即按新默认偏好展示（刷新页面后也走该默认）。
      state.sort = prefs.sort; state.sharedFilter = prefs.shared; state.unreadFilter = prefs.unread;
      state.savedPrefs = {
        sort: typeof saved.sort === 'string' ? saved.sort : prefs.sort,
        shared: saved.shared === 'yes' || saved.shared === 'no' ? saved.shared : 'off',
        unread: saved.unread === 'yes' || saved.unread === 'no' ? saved.unread : 'off',
      };
      clearPrefsUrlParams();
      syncGearBadge();
      state.page = 1;
      load().then(loadTags).catch(() => {});
      closePrefsDropdown();
      toastSuccess('已保存为默认偏好');
    }).catch(() => toastError('保存偏好失败'));
  });
  syncGearBadge();

  // 批量编辑开关（对齐 bulk-edit-active-toggle）。
  document.getElementById('bulk-edit-toggle')?.addEventListener('click', () => setBulkActive(!state.bulkActive));
  document.getElementById('bulk-exit')?.addEventListener('click', () => setBulkActive(false));

  document.getElementById('bulk-select-all')?.addEventListener('change', (e) => {
    const checked = (e.target as HTMLInputElement).checked;
    document.querySelectorAll('#bookmark-list input[type=checkbox]').forEach((cb) => {
      (cb as HTMLInputElement).checked = checked;
      const id = Number((cb as HTMLInputElement).value);
      if (checked) state.selected.add(id); else state.selected.delete(id);
    });
    updateBulkBar();
  });

  const bulkAction = document.getElementById('bulk-action') as HTMLSelectElement | null;
  const bulkTags = document.getElementById('bulk-tag-string') as HTMLInputElement | null;
  bulkAction?.addEventListener('change', () => {
    const needTag = bulkAction.value === 'bulk_tag' || bulkAction.value === 'bulk_untag';
    if (bulkTags) bulkTags.style.display = needTag ? '' : 'none';
  });
  document.getElementById('bulk-execute')?.addEventListener('click', async () => {
    if (!bulkAction) return;
    await runBulkAction(bulkAction.value, bulkTags?.value);
  });

  // 详情弹窗关闭。
  const overlay = document.getElementById('details-overlay');
  overlay?.querySelectorAll('[data-close-modal]').forEach((el) =>
    el.addEventListener('click', () => overlay.classList.remove('active')));

  // 浏览器前进/后退：从 URL 重读状态并重新加载（query 翻页）。
  window.addEventListener('popstate', () => {
    const u = new URL(window.location.href);
    state.q = u.searchParams.get('q') || '';
    state.page = Number(u.searchParams.get('page')) || 1;
    if (qInput && qInput.value !== state.q) qInput.value = state.q;
    load().then(loadTags).catch(() => {});
  });

  // 快捷键 e：切换显示全部备注（对齐上游 shortcuts.js 的 "e"）。
  window.addEventListener('keydown', (e) => {
    if (e.key !== 'e') return;
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable)) return;
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    document.getElementById('bookmark-list')?.classList.toggle('show-notes');
  });

  (async () => {
    try {
      // 渲染偏好来源：登录用户读本人 profile；共享页匿名（401）时读"生效资料"
      // （访客资料 guest_profile_user，对齐上游 request.user_profile 注入）。
      let p: Record<string, unknown> | null = null;
      try {
        p = await get('/api/v1/profile');
      } catch {
        if (mode === 'shared') p = await get('/api/v1/shared/viewer-profile');
      }
      if (!p) return;
      state.dateDisplay = ['absolute', 'hidden'].includes(String(p.bookmark_date_display)) ? (p.bookmark_date_display as 'relative' | 'absolute' | 'hidden') : 'relative';
      state.descDisplay = p.bookmark_description_display === 'separate' ? 'separate' : 'inline';
      state.showUrl = p.display_url !== false;
      // 永久显示备注（后台"永久显示备注"）：为真时列表加 show-notes，全部备注常显（对齐上游 show_notes）。
      state.permanentNotes = !!p.permanent_notes;
      if (state.permanentNotes) document.getElementById('bookmark-list')?.classList.add('show-notes');
      // 书签操作显示开关：匿名访客视图无编辑态，默认不渲染操作按钮。
      state.showViewAction = p.display_view_bookmark_action !== false;
      state.showEditAction = p.display_edit_bookmark_action !== false;
      state.showArchiveAction = p.display_archive_bookmark_action !== false;
      state.showRemoveAction = p.display_remove_bookmark_action !== false;
      // 书签打开方式（后台"书签打开方式"）。
      state.linkTarget = p.bookmark_link_target === '_self' ? '_self' : '_blank';
      // 启用预览图片（后台"启用预览图片"）。
      state.enablePreviewImages = p.enable_preview_images === true;
      // 启用网站图标（后台"启用网站图标"）。
      state.enableFavicons = p.enable_favicons !== false;
      state.tagGrouping = p.tag_grouping === 'alphabetical' ? 'alphabetical' : 'disabled';
      // 固定分页栏（后台"固定分页栏"）→ .bookmark-pagination.sticky。
      if (p.sticky_pagination) document.querySelector('.bookmark-pagination')?.classList.add('sticky');
      // 侧栏偏好：折叠侧边栏 + 隐藏组合（对齐上游 user_profile.collapse_side_panel / hide_bundles）。
      applySidePanelPrefs(!!p.collapse_side_panel, !!p.hide_bundles);
      // 描述最大行数（后台"描述最大行数"设置）→ CSS 行截断变量。
      const maxLines = Number(p.bookmark_description_max_lines);
      if (Number.isFinite(maxLines) && maxLines > 0) {
        document.documentElement.style.setProperty('--ld-bookmark-description-max-lines', String(maxLines));
      }
      if (lastItems.length) renderList(lastItems);
      // tagGrouping 影响标签云渲染；profile 到位后重渲染一次。
      renderTagCloud();
    } catch { /* 默认相对日期 */ }
  })();
  (async () => {
    try {
      if (mode === 'shared') {
        // 匿名共享页先探测登录态：未登录走公开共享接口（对齐 linkding 匿名 /bookmarks/shared）。
        const s = await get('/api/v1/auth/status');
        state.sharedAnon = !(s && s.authenticated);
      }
      await load(); await loadTags(); await loadBundles();
    }
    catch (e: unknown) {
      const status = e instanceof Error && 'status' in e && typeof e.status === 'number' ? e.status : undefined;
      if (status === 401 && state.filter !== 'shared') window.location.href = '/login';
    }
  })();

  // 显示跳转前排队的操作结果提示（创建/编辑书签后回到列表页）。
  showPendingToast();
}
