// SPDX-License-Identifier: AGPL-3.0-or-later

// An info button with a one-sentence explanation: shown on hover and
// focus, and on a tap (phones have no hover). The text is the button's
// accessible description, so screen readers read it with the button.

import { type ReactNode, useId, useState } from "react";
import { Icon } from "./ui";

export function InfoTip({ label, children }: Readonly<{ label: string; children: ReactNode }>) {
  const id = useId();
  const [open, setOpen] = useState(false);
  return (
    <span className={open ? "info-tip open" : "info-tip"}>
      <button
        type="button"
        className="info-tip-button"
        aria-label={label}
        aria-describedby={id}
        aria-expanded={open}
        onClick={() => setOpen(!open)}
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
