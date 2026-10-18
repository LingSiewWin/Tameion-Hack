"use client";
// The hero's code in a chunk of its own, loaded only where the page renders this slot (HOME_HERO=1). Imported directly by
// the page, the hero shared a chunk with the guest card, so a page without the hero still referenced the hero's code.
import dynamic from "next/dynamic";

export const HeroSlot = dynamic(() => import("./hero.tsx").then((m) => m.Hero));
