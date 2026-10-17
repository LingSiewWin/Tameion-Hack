import { redirect } from "next/navigation";
import { hasInvite } from "../../features/onboarding/actions.ts";
import { Stepper } from "../../features/onboarding/stepper.tsx";

// `/new` the stepper: name, passkey, funded, allowance, then the authority receipt (H4). The word comes first, on `/`.
export default async function NewPage() {
  if (!(await hasInvite())) redirect("/");
  return <Stepper />;
}
