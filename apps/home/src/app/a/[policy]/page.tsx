"use client";
import { AgentSkeleton, OwnerSkeleton } from "../../../components/skeletons.tsx";
import { useView } from "../../../lib/view.ts";

// `/a/<policy>` the account, Owner or Agent by the capsule (H2 builds the Owner home, H3 the Agent view, here).
export default function AccountPage() {
  return useView() === "agent" ? <AgentSkeleton /> : <OwnerSkeleton />;
}
