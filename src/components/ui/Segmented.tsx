import { useLayoutEffect, useRef, type CSSProperties, type ReactNode } from "react";

/** Segmented control whose selection pill slides to the child button marked `.on`. */
export function Segmented({ children, style }: { children: ReactNode; style?: CSSProperties }) {
  const box = useRef<HTMLDivElement>(null);
  const ind = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    const on = box.current?.querySelector<HTMLElement>(":scope > button.on");
    const el = ind.current!;
    el.style.opacity = on ? "1" : "0";
    if (on) {
      el.style.width = `${on.offsetWidth}px`;
      el.style.transform = `translateX(${on.offsetLeft - 2}px)`;
    }
  });
  return (
    <div ref={box} className="segmented" style={style}>
      <span ref={ind} className="seg-indicator" aria-hidden="true" />
      {children}
    </div>
  );
}
