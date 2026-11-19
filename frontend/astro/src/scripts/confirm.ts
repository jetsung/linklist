// 自定义确认弹窗（对齐原版 data-confirm 的模态确认，替代原生 confirm）。
// 供书签列表页与标签页等共用。
export function confirmDialog(message: string): Promise<boolean> {
  const { promise, resolve } = Promise.withResolvers<boolean>();
  const overlay = document.createElement('div');
  overlay.className = 'modal modal-confirm active';
  overlay.innerHTML = `
    <div class="modal-overlay"></div>
    <div class="modal-container" role="dialog" aria-modal="true">
      <div class="modal-header"><h2 class="modal-title">确认</h2></div>
      <div class="modal-body"><p>${message}</p></div>
      <div class="modal-footer">
        <div class="actions d-flex justify-center" style="gap:.5rem">
          <button class="btn" type="button" data-act="cancel">取消</button>
          <button class="btn btn-error" type="button" data-act="ok">确定</button>
        </div>
      </div>
    </div>`;
  document.body.append(overlay);
  const done = (v: boolean) => { overlay.remove(); resolve(v); };
  overlay.querySelector('[data-act=ok]')!.addEventListener('click', () => done(true));
  overlay.querySelector('[data-act=cancel]')!.addEventListener('click', () => done(false));
  overlay.querySelector('.modal-overlay')!.addEventListener('click', () => done(false));
  return promise;
}
