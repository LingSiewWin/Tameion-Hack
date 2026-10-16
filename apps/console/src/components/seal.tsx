"use client";

import { motion, useReducedMotion } from "motion/react";
import { cn } from "@/lib/utils";

// The record-keeper's seal (a tameion's chartoularios sealed what he kept): an
// engraved ring whose arc is the one number the owner needs — how much of the
// next stretch of bills is covered. While the agent reads a wallet the arc
// traces; once it knows, it holds still.

const R = 78;
const C = 2 * Math.PI * R;
const TICKS = 96;

export function Seal({
  fraction,
  tone = "protect",
  reading = false,
  size = 220,
  className,
  children,
}: {
  /** 0..1, the covered share; ignored while reading */
  fraction: number;
  tone?: "protect" | "attention" | "muted";
  reading?: boolean;
  size?: number;
  className?: string;
  children?: React.ReactNode;
}) {
  const reduce = useReducedMotion();
  const f = Math.max(0, Math.min(1, fraction));
  const stroke = tone === "protect" ? "var(--protect)" : tone === "attention" ? "var(--attention)" : "var(--muted-foreground)";
  return (
    <div className={cn("relative grid place-items-center", className)} style={{ width: size, height: size }}>
      <svg viewBox="0 0 200 200" className="absolute inset-0 size-full" aria-hidden>
        {/* the engraved edge */}
        <g stroke="currentColor" className="text-border">
          {Array.from({ length: TICKS }, (_, i) => {
            const a = (i / TICKS) * 2 * Math.PI;
            const long = i % 8 === 0;
            const r1 = long ? 89 : 91;
            const r2 = 96;
            return (
              <line
                key={i}
                x1={100 + r1 * Math.sin(a)}
                y1={100 - r1 * Math.cos(a)}
                x2={100 + r2 * Math.sin(a)}
                y2={100 - r2 * Math.cos(a)}
                strokeWidth={long ? 1.4 : 0.8}
              />
            );
          })}
        </g>
        <circle cx="100" cy="100" r={R} fill="none" stroke="var(--border)" strokeWidth="10" />
        {reading ? (
          <motion.circle
            cx="100"
            cy="100"
            r={R}
            fill="none"
            stroke={stroke}
            strokeWidth="10"
            strokeLinecap="round"
            strokeDasharray={`${C * 0.22} ${C}`}
            style={{ originX: "50%", originY: "50%" }}
            animate={reduce ? undefined : { rotate: 360 }}
            transition={{ repeat: Infinity, duration: 1.6, ease: "linear" }}
          />
        ) : (
          <motion.circle
            cx="100"
            cy="100"
            r={R}
            fill="none"
            stroke={stroke}
            strokeWidth="10"
            strokeLinecap="round"
            transform="rotate(-90 100 100)"
            strokeDasharray={C}
            initial={{ strokeDashoffset: C }}
            animate={{ strokeDashoffset: C * (1 - f) }}
            transition={{ duration: reduce ? 0 : 1.1, ease: [0.22, 1, 0.36, 1] }}
          />
        )}
      </svg>
      <div className="relative z-10 flex flex-col items-center text-center">{children}</div>
    </div>
  );
}
