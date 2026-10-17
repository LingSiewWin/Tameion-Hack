import { Suspense } from "react";
import { ReceiveSkeleton } from "../../../../components/skeletons.tsx";
import { Receive } from "../../../../features/onboarding/receive.tsx";

// `/a/<name>/receive` the QR of the account's address (H4). The segment is the account's name (DESIGN §Routes).
export default async function ReceivePage({ params }: { params: Promise<{ name: string }> }) {
  const { name } = await params;
  return (
    <Suspense fallback={<ReceiveSkeleton />}>
      <Receive name={decodeURIComponent(name).toLowerCase()} />
    </Suspense>
  );
}
