// The loading state of every route (DESIGN §States): a skeleton shaped like the layout it stands in for.
// The feature seats render these while their reads are in flight, held at least SKELETON_MIN_MS once shown.
import { Loading, Skeleton } from "./skeleton.tsx";

const CTA = "h-(--cta) w-full";

export function GuestSkeleton() {
  return (
    <Loading label="Loading">
      <div className="flex flex-col gap-(--space-5)">
        <Skeleton className="h-12 w-full" />
        <Skeleton className="h-12 w-2/3" />
        <Skeleton className={CTA} />
        <Skeleton className={CTA} />
      </div>
    </Loading>
  );
}

/** Owner home: the authority sentence and Stop, four facts, then what the agent paid. */
export function OwnerSkeleton() {
  return (
    <Loading label="Loading the account">
      <div className="flex flex-col gap-(--space-6)">
        <div className="flex flex-col gap-(--space-3)">
          <Skeleton className="h-12 w-full" />
          <Skeleton className="h-12 w-3/4" />
          <Skeleton className={`${CTA} mt-(--space-3)`} />
        </div>
        <div className="grid grid-cols-2 gap-(--space-3)">
          {[0, 1, 2, 3].map((i) => (
            <Skeleton key={i} className="h-24" />
          ))}
        </div>
        <div className="flex flex-col gap-(--space-3)">
          <Skeleton className="h-[22px] w-56" />
          {[0, 1, 2].map((i) => (
            <Skeleton key={i} className="h-16 w-full" />
          ))}
        </div>
      </div>
    </Loading>
  );
}

/** Agent view: the `tameion top` table, then the record stream, full width. */
export function AgentSkeleton() {
  return (
    <Loading label="Loading the account">
      <div className="flex flex-col gap-(--space-2)">
        <Skeleton className="h-5 w-full" />
        <Skeleton className="h-5 w-full" />
        <div className="h-(--space-5)" />
        {[0, 1, 2, 3, 4, 5].map((i) => (
          <Skeleton key={i} className="h-5 w-full" />
        ))}
      </div>
    </Loading>
  );
}

export function StepperSkeleton() {
  return (
    <Loading label="Loading">
      <div className="flex flex-col gap-(--space-4)">
        <Skeleton className="h-[18px] w-24" />
        <Skeleton className="h-12 w-3/4" />
        <Skeleton className={CTA} />
        <Skeleton className={CTA} />
      </div>
    </Loading>
  );
}

/** Add money: one plate per way (the card is not built, so it has no plate). */
export function AddMoneySkeleton() {
  return (
    <Loading label="Loading">
      <div className="flex flex-col gap-(--space-4)">
        <Skeleton className="h-12 w-2/3" />
        <Skeleton className="h-40 w-full" />
        <Skeleton className="h-40 w-full" />
      </div>
    </Loading>
  );
}

export function ReceiveSkeleton() {
  return (
    <Loading label="Loading">
      <div className="flex flex-col items-center gap-(--space-4)">
        <Skeleton className="size-60" />
        <Skeleton className="h-[22px] w-48" />
        <div className="flex w-full gap-(--space-3)">
          <Skeleton className="h-(--cta) flex-1" />
          <Skeleton className="h-(--cta) flex-1" />
        </div>
      </div>
    </Loading>
  );
}
