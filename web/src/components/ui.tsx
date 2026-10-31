// SPDX-License-Identifier: AGPL-3.0-or-later

// Building blocks shared by the pages.

import {
  type ReactNode,
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
} from "react";
import { errorMessage } from "../api";
import { Breadcrumb } from "./Breadcrumb";
import { t } from "../i18n";

// Icons (24 px grid, stroked) ---------------------------------------------

const ICONS = {
  air: "M4.9 19.1a10 10 0 0 1 0-14.2M7.8 16.2a6 6 0 0 1 0-8.4M16.2 7.8a6 6 0 0 1 0 8.4M19.1 4.9a10 10 0 0 1 0 14.2M12 13a1 1 0 1 0 0-2 1 1 0 0 0 0 2Z",
  queue: "M3 6h13M3 12h13M3 18h9M19 15v6M16 18h6",
  library: "M4 4h4v16H4zM10 4h4v16h-4zM16.5 4.5l3.8 1 -3.9 15-3.8-1z",
  tag: "M3 12V4a1 1 0 0 1 1-1h8l9 9-9 9zM7.5 7.5h.01",
  label: "M6 3h12v18l-6-4-6 4z",
  review: "M9 11l3 3 8-8M20 12v7a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h9",
  history: "M3 12a9 9 0 1 0 3-6.7L3 8M3 3v5h5M12 7v5l3 2",
  fade: "M3 6c6 0 9 12 18 12M3 18c9 0 12-12 18-12",
  output: "M5 12.55a11 11 0 0 1 14 0M8.5 16.1a6 6 0 0 1 7 0M2 8.8a16 16 0 0 1 20 0M12 20h.01",
  settings:
    "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6ZM19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1Z",
  users:
    "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8ZM22 21v-2a4 4 0 0 0-3-3.9M16 3.1a4 4 0 0 1 0 7.8",
  user: "M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2M12 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8Z",
  skip: "M5 4l10 8-10 8zM19 5v14",
  pause: "M6 4h4v16H6zM14 4h4v16h-4z",
  play: "M6 4l14 8-14 8z",
  stop: "M6 6h12v12H6z",
  plus: "M12 5v14M5 12h14",
  eq: "M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6",
  next: "M4 12h12M12 6l6 6-6 6M20 5v14",
  trash: "M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6",
  close: "M18 6 6 18M6 6l12 12",
  grip: "M9 6h.01M9 12h.01M9 18h.01M15 6h.01M15 12h.01M15 18h.01",
  menu: "M3 6h18M3 12h18M3 18h18",
  warn: "M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0ZM12 9v4M12 17h.01",
  check: "M20 6 9 17l-5-5",
  search: "M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16ZM21 21l-4.3-4.3",
  undo: "M3 7v6h6M21 17a9 9 0 0 0-15-6.7L3 13",
  sun: "M12 17a5 5 0 1 0 0-10 5 5 0 0 0 0 10ZM12 1v2M12 21v2M4.2 4.2l1.4 1.4M18.4 18.4l1.4 1.4M1 12h2M21 12h2M4.2 19.8l1.4-1.4M18.4 5.6l1.4-1.4",
  moon: "M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8Z",
  monitor: "M3 4h18v12H3zM8 20h8M12 16v4",
  logout: "M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4M16 17l5-5-5-5M21 12H9",
  edit: "M12 20h9M16.5 3.5a2.1 2.1 0 1 1 3 3L7 19l-4 1 1-4Z",
  bolt: "M13 2 3 14h9l-1 8 10-12h-9z",
  clock: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18ZM12 7v5h4",
  calendar: "M4 5h16v16H4zM4 10h16M8 3v4M16 3v4",
  ban: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18ZM5.7 5.7l12.6 12.6",
  mic: "M12 15a3 3 0 0 0 3-3V5a3 3 0 0 0-6 0v7a3 3 0 0 0 3 3ZM19 11a7 7 0 0 1-14 0M12 18v3",
  log: "M9 6h12M9 12h12M9 18h12M4 6h.01M4 12h.01M4 18h.01",
  upload: "M12 16V4M7 9l5-5 5 5M4 20h16",
  download: "M12 4v12M7 11l5 5 5-5M4 20h16",
  chart: "M4 20V10M10 20V4M16 20v-7M22 20H2",
  link: "M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1",
} as const;

export type IconName = keyof typeof ICONS;

export function Icon({ name, size = 18, label }: { name: IconName; size?: number; label?: string }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      <path d={ICONS[name]} />
    </svg>
  );
}

export function Logo({ size = 24 }: { size?: number }) {
  // A heron's head and beak over a wave.
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" fill="none" aria-hidden="true">
      <path
        d="M9 21c0-6 3-11 9-12l10-3-9 7c-3 2-4 5-4 8"
        stroke="currentColor"
        strokeWidth={2.4}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <circle cx="17" cy="11.5" r="1.3" fill="currentColor" />
      <path
        d="M3 26c2.5-2 5-2 7.5 0s5 2 7.5 0 5-2 7.5 0 3 1.5 4 1"
        stroke="currentColor"
        strokeWidth={2}
        strokeLinecap="round"
      />
    </svg>
  );
}

// Layout --------------------------------------------------------------------

export function Card({
  title,
  actions,
  children,
  flush,
  className,
}: {
  title?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  flush?: boolean;
  className?: string;
}) {
  return (
    <section className={`card ${className ?? ""}`}>
      {(title !== undefined || actions !== undefined) && (
        <div className="card-head">
          {typeof title === "string" ? <h2>{title}</h2> : title}
          {actions && <div className="row">{actions}</div>}
        </div>
      )}
      <div className={flush ? "card-body flush" : "card-body"}>{children}</div>
    </section>
  );
}

export function PageHead({
  title,
  description,
  actions,
}: {
  title: string;
  description?: ReactNode;
  actions?: ReactNode;
}) {
  useEffect(() => {
    document.title = t("app.title", { page: title });
  }, [title]);
  return (
    <header className="page-head">
      <div>
        <Breadcrumb title={title} />
        <h1>{title}</h1>
        {description && <p className="muted">{description}</p>}
      </div>
      {actions && <div className="row">{actions}</div>}
    </header>
  );
}

export function Banner({ kind, children }: { kind: "warn" | "bad"; children: ReactNode }) {
  return (
    <div className={`banner ${kind}`} role={kind === "bad" ? "alert" : "status"}>
      <Icon name="warn" />
      <div>{children}</div>
    </div>
  );
}

export function Loading({ label = t("ui.loading") }: { label?: string }) {
  return (
    <div className="empty row" style={{ justifyContent: "center" }}>
      <span className="spinner" aria-hidden="true" /> {label}
    </div>
  );
}

export function ErrorView({ error }: { error: unknown }) {
  return <Banner kind="bad">{errorMessage(error)}</Banner>;
}

export function Empty({ children }: { children: ReactNode }) {
  return <div className="empty">{children}</div>;
}

// Forms ---------------------------------------------------------------------

export function Field({
  label,
  hint,
  children,
}: {
  label: ReactNode;
  hint?: ReactNode;
  children: ReactNode;
}) {
  return (
    <label className="field">
      <span>{label}</span>
      {children}
      {hint && <span className="hint">{hint}</span>}
    </label>
  );
}

/** A number input that keeps what is typed until it is a valid number. */
export function NumberInput({
  value,
  onChange,
  min,
  max,
  step,
  disabled,
  suffix,
  label,
}: {
  value: number;
  onChange: (value: number) => void;
  min?: number;
  max?: number;
  step?: number;
  disabled?: boolean;
  suffix?: string;
  /** Its accessible name, when no visible label names it. */
  label?: string;
}) {
  const [text, setText] = useState(String(value));
  useEffect(() => {
    setText((current) => (Number(current) === value ? current : String(value)));
  }, [value]);
  const parsed = Number(text);
  const valid =
    text.trim() !== "" &&
    Number.isFinite(parsed) &&
    (min === undefined || parsed >= min) &&
    (max === undefined || parsed <= max);
  const input = (
    <input
      className={valid ? "input num" : "input num invalid"}
      type="number"
      inputMode="decimal"
      value={text}
      min={min}
      max={max}
      step={step ?? "any"}
      disabled={disabled}
      aria-label={label}
      aria-invalid={!valid}
      onChange={(event) => {
        setText(event.target.value);
        const next = Number(event.target.value);
        if (
          event.target.value.trim() !== "" &&
          Number.isFinite(next) &&
          (min === undefined || next >= min) &&
          (max === undefined || next <= max)
        ) {
          onChange(next);
        }
      }}
    />
  );
  if (!suffix) {
    return input;
  }
  return (
    <span className="row" style={{ flexWrap: "nowrap" }}>
      {input}
      <span className="muted small">{suffix}</span>
    </span>
  );
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
  disabled,
}: {
  value: T;
  options: { value: T; label: ReactNode }[];
  onChange: (value: T) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <div className="segmented" role="group" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          aria-pressed={option.value === value}
          disabled={disabled}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

// Dialogs -------------------------------------------------------------------

export function Modal({
  title,
  open,
  onClose,
  children,
  footer,
  wide,
}: {
  title: string;
  open: boolean;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  wide?: boolean;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) {
      return;
    }
    if (open && !dialog.open) {
      dialog.showModal();
    } else if (!open && dialog.open) {
      dialog.close();
    }
  }, [open]);
  return (
    <dialog
      ref={ref}
      className={wide ? "modal wide" : "modal"}
      onClose={onClose}
      onCancel={onClose}
      aria-labelledby="modal-title"
    >
      {open && (
        <>
          <div className="modal-head">
            <h2 id="modal-title">{title}</h2>
            <button type="button" className="btn ghost icon" onClick={onClose} aria-label={t("ui.close")}>
              <Icon name="close" />
            </button>
          </div>
          <div className="modal-body">{children}</div>
          {footer && <div className="modal-foot">{footer}</div>}
        </>
      )}
    </dialog>
  );
}

/** A button that asks before doing something hard to take back. */
export function ConfirmButton({
  children,
  question,
  confirm,
  onConfirm,
  className,
  disabled,
  title,
}: {
  children: ReactNode;
  question: ReactNode;
  confirm: string;
  onConfirm: () => void;
  className?: string;
  disabled?: boolean;
  title?: string;
}) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button
        type="button"
        className={className ?? "btn danger"}
        disabled={disabled}
        title={title}
        aria-label={title}
        onClick={() => setOpen(true)}
      >
        {children}
      </button>
      <ConfirmDialog
        open={open}
        onClose={() => setOpen(false)}
        question={question}
        confirm={confirm}
        onConfirm={onConfirm}
      />
    </>
  );
}

/** Asks before doing something hard to take back, opened by its caller. */
export function ConfirmDialog({
  open,
  onClose,
  question,
  confirm,
  onConfirm,
}: {
  open: boolean;
  onClose: () => void;
  question: ReactNode;
  confirm: string;
  onConfirm: () => void;
}) {
  return (
    <Modal
      title={t("ui.areYouSure")}
      open={open}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("ui.cancel")}
          </button>
          <button
            type="button"
            className="btn danger solid"
            onClick={() => {
              onClose();
              onConfirm();
            }}
          >
            {confirm}
          </button>
        </>
      }
    >
      {question}
    </Modal>
  );
}

// Toasts --------------------------------------------------------------------

interface Toast {
  id: number;
  text: string;
  bad: boolean;
}

const ToastContext = createContext<(text: string, bad?: boolean) => void>(() => {});

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const next = useRef(1);
  const show = useCallback((text: string, bad = false) => {
    const id = next.current++;
    setToasts((current) => [...current.slice(-3), { id, text, bad }]);
    setTimeout(
      () => setToasts((current) => current.filter((toast) => toast.id !== id)),
      bad ? 7000 : 3500,
    );
  }, []);
  return (
    <ToastContext.Provider value={show}>
      {children}
      <div className="toasts" aria-live="polite">
        {toasts.map((toast) => (
          <div key={toast.id} className={toast.bad ? "toast bad" : "toast"} role="status">
            {toast.text}
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

/** Shows a short message: `toast("Saved")`, `toast(message, true)` for errors. */
export function useToast() {
  return useContext(ToastContext);
}
