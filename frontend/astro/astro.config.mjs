import { defineConfig } from 'astro/config';

// 静态多页输出：构建产物直接落在 rust-embed 嵌入的 frontend/dist。
export default defineConfig({
  output: 'static',
  outDir: '../dist',
  // 产物进 monorepo frontend/dist；清空默认 public 处理。
  build: { assets: 'assets' },
  devToolbar: { enabled: false },
});
