import { monoPreload } from "../mono-preload.ts";

// The stepper keeps Plex Mono preloaded (see ../mono-preload.ts); the wrapper takes no box.
export default function NewLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <div className={monoPreload.variable} style={{ display: "contents" }}>
      {children}
    </div>
  );
}
