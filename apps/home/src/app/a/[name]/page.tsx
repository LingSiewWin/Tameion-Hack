"use client";
import { AgentSkeleton, OwnerSkeleton } from "../../../components/skeletons.tsx";
import { useView } from "../../../lib/view.ts";

// `/a/<name>` the account by its name, Owner or Agent by the capsule (H2 builds the Owner home, H3 the Agent view, here).
// fetchName (src/lib/api.ts) turns the name into the address for API calls; the address never reaches an href or the Owner DOM.
export default function AccountPage() {
  return useView() === "agent" ? <AgentSkeleton /> : <OwnerSkeleton />;
}
