"use client";
// `/` the guest (H4): the word from the judges' form, checked on the server (actions.ts `enterGuest`), opens the seeded
// demo account read-only or starts the stepper. Two buttons on one field; it works without JavaScript too.
import { useActionState } from "react";
import { enterGuest, type GuestState } from "./actions.ts";
import { CTA, CTA_QUIET, FIELD } from "./ui.tsx";

export function Guest() {
  const [state, act, pending] = useActionState<GuestState, FormData>(enterGuest, {});
  return (
    <form action={act} className="flex flex-col gap-(--space-5)">
      <h1 className="statement-line">Your agents pay their bills around the clock in USDC, up to the mandate you signed.</h1>
      <div className="flex flex-col gap-(--space-2)">
        <label htmlFor="word" className="row-title">
          The word from the judges&apos; form
        </label>
        <input
          id="word"
          name="word"
          type="text"
          required
          autoComplete="off"
          autoCapitalize="none"
          spellCheck={false}
          aria-invalid={!!state.error}
          aria-describedby={state.error ? "word-error" : undefined}
          className={FIELD}
        />
        {state.error && (
          <p id="word-error" role="alert" className="row text-(--red)">
            {state.error}
          </p>
        )}
      </div>
      <div className="flex flex-col gap-(--space-3)">
        <button type="submit" name="intent" value="demo" className={CTA} aria-busy={pending}>
          Open the demo account
        </button>
        <button type="submit" name="intent" value="new" className={CTA_QUIET} aria-busy={pending}>
          Make your own
        </button>
      </div>
    </form>
  );
}
