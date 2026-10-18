// The frame every route renders inside: the header, then the surface. Owner content is 720 px wide, Agent full width.
import { Header } from "./header.tsx";

export function Shell({ children }: { children: React.ReactNode }) {
  return (
    <>
      <Header />
      <main className="surface py-(--space-5)">{children}</main>
    </>
  );
}
