"use client";

import { useCallback, useRef, useState } from "react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";

type Ask = { title: string; body: React.ReactNode; action: string };

/** A trust decision shown in words before the passkey is asked: what will be
 *  paid, to whom, and for how long the approval holds. */
export function useConfirm() {
  const [ask, setAsk] = useState<Ask | null>(null);
  const done = useRef<(ok: boolean) => void>(() => {});
  const confirm = useCallback(
    (a: Ask) =>
      new Promise<boolean>((resolve) => {
        done.current = resolve;
        setAsk(a);
      }),
    [],
  );
  const close = (ok: boolean) => {
    done.current(ok);
    setAsk(null);
  };
  const dialog = (
    <AlertDialog open={!!ask} onOpenChange={(o) => !o && close(false)}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{ask?.title}</AlertDialogTitle>
          <AlertDialogDescription asChild>
            <div className="text-sm text-muted-foreground">{ask?.body}</div>
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel onClick={() => close(false)}>Not now</AlertDialogCancel>
          <AlertDialogAction onClick={() => close(true)}>{ask?.action}</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
  return { confirm, dialog };
}
