import { useToastStore } from '../state/toast-store';
import { joinClassNames } from './Button';

export function ToastViewport() {
  const toasts = useToastStore((state) => state.toasts);
  const dismiss = useToastStore((state) => state.dismiss);

  if (toasts.length === 0) {
    return null;
  }

  return (
    <div className="toast-viewport" aria-live="polite">
      {toasts.map((toast) => (
        <div
          key={toast.id}
          role={toast.kind === 'error' ? 'alert' : 'status'}
          className={joinClassNames('toast', toast.kind === 'error' ? 'toast-error' : 'toast-info')}
        >
          <div className="toast-body">
            <strong>{toast.title}</strong>
            {toast.detail ? <span>{toast.detail}</span> : null}
          </div>
          <button type="button" className="link-button" onClick={() => dismiss(toast.id)}>
            Dismiss
          </button>
        </div>
      ))}
    </div>
  );
}
