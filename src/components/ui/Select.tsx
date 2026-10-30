import { Children, isValidElement, useEffect, useLayoutEffect, useRef, useState, type ReactElement, type ReactNode } from "react";
import { createPortal } from "react-dom";

type Item = { value: string; label: string; disabled?: boolean } | { group: string };

/** Drop-in for <select>: same <option>/<optgroup> children and `onChange(e.target.value)`, macOS popup-button look. */
export function Select({ value, onChange, disabled, className = "", children, style, "aria-label": ariaLabel }: {
  value: string | number; onChange: (e: { target: { value: string } }) => void; disabled?: boolean; className?: string;
  children: ReactNode; style?: React.CSSProperties; "aria-label"?: string;
}) {
  const items = flatten(children);
  const options = items.filter((i): i is Extract<Item, { value: string }> => "value" in i);
  const current = options.find((o) => o.value === String(value)) ?? options[0];
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [pos, setPos] = useState<{ left: number; top: number; width: number; up: boolean; max: number } | null>(null);
  const btn = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);

  const place = () => {
    const r = btn.current!.getBoundingClientRect();
    const below = innerHeight - r.bottom - 12, above = r.top - 12;
    const up = below < 220 && above > below;
    setPos({ left: r.left, top: up ? r.top - 4 : r.bottom + 4, width: r.width, up, max: Math.min(320, up ? above : below) });
  };
  const show = () => { if (disabled) return; place(); setActive(options.indexOf(current)); setOpen(true); };
  const pick = (v: string) => { setOpen(false); btn.current?.focus(); if (v !== String(value)) onChange({ target: { value: v } }); };

  useEffect(() => {
    if (!open) return;
    const close = (e: Event) => { if (!menu.current?.contains(e.target as Node) && !btn.current?.contains(e.target as Node)) setOpen(false); };
    const away = () => setOpen(false);
    addEventListener("mousedown", close, true);
    addEventListener("resize", away);
    addEventListener("blur", away);
    return () => { removeEventListener("mousedown", close, true); removeEventListener("resize", away); removeEventListener("blur", away); };
  }, [open]);
  useLayoutEffect(() => { if (open) menu.current?.querySelector<HTMLElement>(".menu-item.active")?.scrollIntoView({ block: "nearest" }); }, [open, active]);

  const step = (d: number) => {
    let i = active;
    for (let n = 0; n < options.length; n++) {
      i = (i + d + options.length) % options.length;
      if (!options[i].disabled) return setActive(i);
    }
  };
  const onKey = (e: React.KeyboardEvent) => {
    if (!open) {
      if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key)) { e.preventDefault(); show(); }
      return;
    }
    if (e.key === "Escape") { e.preventDefault(); setOpen(false); }
    else if (e.key === "ArrowDown") { e.preventDefault(); step(1); }
    else if (e.key === "ArrowUp") { e.preventDefault(); step(-1); }
    else if (e.key === "Enter" || e.key === " ") { e.preventDefault(); if (options[active]) pick(options[active].value); }
    else if (e.key === "Tab") setOpen(false);
    else if (e.key.length === 1) {
      const i = options.findIndex((o) => !o.disabled && o.label.toLowerCase().startsWith(e.key.toLowerCase()));
      if (i >= 0) setActive(i);
    }
  };

  return (
    <>
      <button ref={btn} type="button" className={`select ${className}`} style={style} disabled={disabled} aria-haspopup="listbox" aria-expanded={open}
        aria-label={ariaLabel} onClick={() => (open ? setOpen(false) : show())} onKeyDown={onKey}>
        <span className="select-label">{current?.label ?? ""}</span>
      </button>
      {open && pos && createPortal(
        <div ref={menu} role="listbox" className={`menu ${pos.up ? "up" : ""}`} onKeyDown={onKey}
          style={{ left: pos.left, minWidth: pos.width, maxHeight: pos.max, ...(pos.up ? { bottom: innerHeight - pos.top } : { top: pos.top }) }}>
          {items.map((it, i) => "group" in it
            ? <div key={`g${i}`} className="menu-group">{it.group}</div>
            : (
              <div key={`o${i}`} role="option" aria-selected={it.value === current?.value} aria-disabled={it.disabled}
                className={`menu-item ${options[active] === it ? "active" : ""} ${it.disabled ? "disabled" : ""}`}
                onMouseEnter={() => setActive(options.indexOf(it))} onClick={() => !it.disabled && pick(it.value)}>
                <span className="menu-check">{it.value === current?.value ? "✓" : ""}</span>{it.label}
              </div>
            ))}
        </div>,
        document.body,
      )}
    </>
  );
}

function text(n: ReactNode): string {
  if (n == null || typeof n === "boolean") return "";
  if (typeof n === "string" || typeof n === "number") return String(n);
  if (Array.isArray(n)) return n.map(text).join("");
  if (isValidElement(n)) return text((n.props as { children?: ReactNode }).children);
  return "";
}

function flatten(children: ReactNode, out: Item[] = []): Item[] {
  Children.forEach(children, (c) => {
    if (!isValidElement(c)) return;
    const el = c as ReactElement<{ value?: string | number; label?: string; disabled?: boolean; children?: ReactNode }>;
    if (el.type === "option") {
      const label = text(el.props.children);
      out.push({ value: String(el.props.value ?? label), label, disabled: el.props.disabled });
    } else if (el.type === "optgroup") {
      out.push({ group: el.props.label ?? "" });
      flatten(el.props.children, out);
    } else {
      flatten(el.props.children, out);
    }
  });
  return out;
}
