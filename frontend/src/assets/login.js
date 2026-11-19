import { post } from './api.js';

document.getElementById('login-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const msg = document.getElementById('msg');
  msg.textContent = '';
  const username = document.getElementById('username').value.trim();
  const password = document.getElementById('password').value;
  try {
    // 首次使用：status 未初始化时跳 setup。
    const status = await fetch('/api/v1/auth/status').then((r) => r.json());
    if (status && status.needs_setup) {
      await post('/api/v1/auth/setup', { username, password });
    }
    await post('/api/v1/auth/login', { username, password });
    location.href = '/bookmarks';
  } catch (err) {
    msg.textContent = err.message || '登录失败';
  }
});
