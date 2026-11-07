// SPDX-License-Identifier: AGPL-3.0-or-later

// An info button with a one-sentence explanation: shown on hover and
// focus, and on a tap (phones have no hover). The text is the button's
// accessible description, so screen readers read it with the button. It
// opens toward the side of the screen with room for it.

import { type ReactNode, useId, useState } from "react";
import { Icon } from "./ui";

export function InfoTip({ label, children }: Readonly<{ label: string; children: ReactNode }>) {
  const id = useId();
  const [open, setOpen] = useState(false);
  const [side, setSide] = useState<"left" | "right">("left");
  const place = (element: HTMLElement) => setSide(element.getBoundingClientRect().left > window.innerWidth / 2 ? "right" : "left");
  return (
    <span className={`info-tip ${side}${open ? " open" : ""}`} onPointerEnter={(e) => place(e.currentTarget)}>
      <button
        type="button"
        className="info-tip-button"
        aria-label={label}
        aria-describedby={id}
        aria-expanded={open}
        onClick={(e) => {
          place(e.currentTarget);
          setOpen(!open);
        }}
        onFocus={(e) => place(e.currentTarget)}
        onBlur={() => setOpen(false)}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            setOpen(false);
          }
        }}
      >
        <Icon name="info" size={14} />
      </button>
      <span role="tooltip" id={id} className="info-tip-text">
        {children}
      </span>
    </span>
  );
}
