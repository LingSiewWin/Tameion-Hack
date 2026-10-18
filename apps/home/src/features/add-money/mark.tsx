// A real mark (a token or a chain) at 32 px: the USDC token logo's minimum on screen, the size every mark takes here, in
// its own colours (DESIGN). Decorative: the words beside it name the token or chain. Sources: public/marks/SOURCES.txt.
export function Mark({ src }: { src: string }) {
  // eslint-disable-next-line @next/next/no-img-element -- a 32 px static SVG from public/; next/image adds nothing here
  return <img src={src} alt="" width={32} height={32} className="size-8 shrink-0" />;
}
