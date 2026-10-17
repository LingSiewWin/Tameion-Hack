"use client";
import { useParams } from "next/navigation";
import { AgentView } from "../../../features/agent/agent-view.tsx";
import { OwnerHome } from "../../../features/owner/owner-home.tsx";
import { useView } from "../../../lib/view.ts";

// `/a/<name>` the account by its name, Owner or Agent by the capsule (H2 builds the Owner home, H3 the Agent view, here).
// fetchName (src/lib/api.ts) turns the name into the address for API calls; the address never reaches an href or the Owner DOM.
export default function AccountPage() {
  const { name } = useParams<{ name: string }>();
  return useView() === "agent" ? <AgentView name={name} /> : <OwnerHome name={decodeURIComponent(name)} />;
}
