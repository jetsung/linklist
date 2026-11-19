// 全局 Toast 通知：所有增删改查反馈统一走这里，不再向页面插入提示元素。
// 展示形态：视口正中央、语义配色的提示卡片（参考 Carbon/Fluent/MUI Snackbar 通用设计：
// 浅色底 + 状态色竖条与文字 + 边框 + 投影，min-width 320px / max-width 560px）。
// 增删改类操作（blocking）额外挂一层透明遮罩并拦截页面全部点击/键盘触发，底层不可
// 操作；Toast 关闭（手动或超时）后卸载拦截器，页面恢复可点。只读类反馈不遮罩。
// 同一时刻只保留一条提示（后到替换先到）。

export type ToastType = 'success' | 'error' | 'warning';

export interface ToastOptions {
  /** 提示类型，默认 success。 */
  type?: ToastType;
  /** 自动消失毫秒数，默认 4000；0 表示不自动消失（只能点关闭）。 */
  timeout?: number;
  /** 是否带透明遮罩并阻断页面交互（增删改类操作置 true）。 */
  blocking?: boolean;
}

const OVERLAY_ID = 'global-toast-overlay';
const PLAIN_ID = 'global-toast-plain';

const TOAST_STYLES = `
#${OVERLAY_ID} {
  position: fixed;
  inset: 0;
  z-index: 300;
  display: flex;
  align-items: center;
  justify-content: center;
  background: transparent;
}
#${OVERLAY_ID},
#${PLAIN_ID} {
  animation: toast-fade 0.15s ease-out;
}
#${PLAIN_ID} {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  z-index: 300;
  pointer-events: none;
}
.toast {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  margin: 0;
  min-width: 20rem;
  max-width: 35rem;
  padding: 0.9rem 1rem;
  border: 1px solid var(--border-color, #d1d5db);
  border-left-width: 4px;
  border-radius: 8px;
  background: var(--body-color, #fff);
  color: var(--text-color, #374151);
  font-size: 0.9rem;
  line-height: 1.4;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.12), 0 2px 6px rgba(0, 0, 0, 0.08);
  animation: toast-in 0.18s ease-out;
  pointer-events: auto;
}
.toast-success {
  border-color: var(--success-color, hsl(142, 76%, 36%));
  border-left-color: var(--success-color, hsl(142, 76%, 36%));
  background: var(--success-color-highlight, hsl(142, 76%, 40%));
  color: #fff;
}
.toast-error {
  border-color: var(--error-color, hsl(0, 72%, 51%));
  border-left-color: var(--error-color, hsl(0, 72%, 51%));
  background: var(--error-color, hsl(0, 72%, 51%));
  color: #fff;
}
.toast-warning {
  border-color: var(--warning-color, hsl(38, 92%, 50%));
  border-left-color: var(--warning-color, hsl(38, 92%, 50%));
  background: var(--warning-color, hsl(38, 92%, 50%));
  color: #1f2937;
}
.toast .toast-text {
  flex: 1;
  word-break: break-word;
}
.toast .toast-close {
  flex: none;
  width: 1.4rem;
  height: 1.4rem;
  padding: 0;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: inherit;
  font-size: 1rem;
  line-height: 1;
  cursor: pointer;
  opacity: 0.7;
}
.toast .toast-close:hover { opacity: 1; }
@keyframes toast-in {
  from { opacity: 0; transform: scale(0.96); }
  to { opacity: 1; transform: scale(1); }
}
@keyframes toast-fade {
  from { opacity: 0; }
  to { opacity: 1; }
}
`;

let activeToast: HTMLElement | null = null;
let hideTimer: number | null = null;

const BLOCKED_EVENTS = ['click', 'mousedown', 'mouseup', 'touchstart', 'touchend', 'keydown', 'keyup', 'submit', 'wheel', 'touchmove', 'scroll'];

/** 遮罩期间拦截一切点击/触摸/键盘触发（捕获阶段，先于页面处理器执行）；toast 自身放行。 */
function blockEvents(e: Event): void {
  if (activeToast?.contains(e.target as Node)) return;
  e.preventDefault();
  e.stopImmediatePropagation();
}

/** 锁定/恢复页面滚动（遮罩期间底层不可滚动，对齐模态行为）。 */
let savedOverflow = '';
function lockPageScroll(): void {
  if (document.body.style.overflow !== 'hidden') savedOverflow = document.body.style.overflow;
  document.body.style.overflow = 'hidden';
}
function unlockPageScroll(): void {
  document.body.style.overflow = savedOverflow;
  savedOverflow = '';
}

function installCaptureBlock(): void {
  for (const type of BLOCKED_EVENTS) {
    document.addEventListener(type, blockEvents, { capture: true });
  }
  lockPageScroll();
}

function uninstallCaptureBlock(): void {
  for (const type of BLOCKED_EVENTS) {
    document.removeEventListener(type, blockEvents, { capture: true });
  }
  unlockPageScroll();
}

function mountStyles(): void {
  if (document.getElementById('global-toast-styles')) return;
  const style = document.createElement('style');
  style.id = 'global-toast-styles';
  style.textContent = TOAST_STYLES;
  document.head.append(style);
}

function ensureHost(blocking: boolean): HTMLElement {
  mountStyles();
  const hostId = blocking ? OVERLAY_ID : PLAIN_ID;
  let host = document.getElementById(hostId);
  if (!host) {
    host = document.createElement('div');
    host.id = hostId;
    document.body.append(host);
  }
  if (blocking) installCaptureBlock();
  return host;
}

function removeToast(): void {
  uninstallCaptureBlock();
  document.getElementById(OVERLAY_ID)?.remove();
  document.getElementById(PLAIN_ID)?.remove();
  activeToast = null;
}

/** 显示一条 Toast（视口居中）；同一时刻只有一条，重复调用会替换当前提示。 */
export function showToast(message: string, options: ToastOptions = {}): void {
  const { type = 'success', timeout = 4000, blocking = false } = options;
  const host = ensureHost(blocking);
  // 单例：清掉当前提示与定时器，新提示顶替。
  clearTimeout(hideTimer);
  hideTimer = null;
  activeToast?.remove();

  const toast = document.createElement('div');
  toast.className = `toast toast-${type}`;
  if (blocking) {
    toast.setAttribute('role', 'alertdialog');
    toast.setAttribute('aria-modal', 'true');
  } else {
    toast.setAttribute('role', 'status');
  }
  const text = document.createElement('span');
  text.className = 'toast-text';
  text.textContent = message;
  const close = document.createElement('button');
  close.className = 'toast-close';
  close.textContent = '×';
  close.setAttribute('aria-label', '关闭');
  close.addEventListener('click', () => { clearTimeout(hideTimer); removeToast(); });
  toast.append(text, close);
  host.replaceChildren(toast);
  activeToast = toast;
  if (timeout > 0) hideTimer = setTimeout(removeToast, timeout);
}

/** 操作成功提示；增删改类调用请传 { blocking: true }。 */
export const toastSuccess = (msg: string, options: ToastOptions = {}) => showToast(msg, { ...options, type: 'success' });
/** 操作失败提示；增删改类调用请传 { blocking: true }。 */
export const toastError = (msg: string, options: ToastOptions = {}) => showToast(msg, { ...options, type: 'error' });

const PENDING_KEY = 'pending-toast';

/** 把提示存入 sessionStorage，跳转后由目标页 showPendingToast() 显示（避免跳转丢失）。 */
export function queueToast(message: string, type: ToastType = 'success'): void {
  try { sessionStorage.setItem(PENDING_KEY, JSON.stringify({ message, type, blocking: true })); } catch { /* 忽略 */ }
}

/** 页面加载时显示跳转前排队的提示（如有）。 */
export function showPendingToast(): void {
  try {
    const raw = sessionStorage.getItem(PENDING_KEY);
    if (!raw) return;
    sessionStorage.removeItem(PENDING_KEY);
    const { message, type } = JSON.parse(raw) as { message: string; type: ToastType };
    showToast(message, { type, blocking: true });
  } catch { /* 忽略 */ }
}
