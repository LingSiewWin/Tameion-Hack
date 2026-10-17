"use client";
// The guest card on `/` (H4), below the hero H7 builds: the word from the judges' form, checked on the server
// (actions.ts `enterGuest`), opens the seeded demo account read-only or starts the stepper. Two actions on one field;
// it works without JavaScript too. `id="guest"` is where the hero's actions point.
import { useActionState } from "react";
import { enterGuest, type GuestState } from "./actions.ts";
import { guest } from "./copy.ts";
import { CTA, CTA_QUIET, FIELD, PLATE, T } from "./styles.ts";

export function GuestCard() {
  const [state, act, pending] = useActionState<GuestState, FormData>(enterGuest, {});
  return (
    <section id="guest" aria-labelledby="guest-title" className={`${PLATE} max-w-(--measure)`}>
      <form action={act} className="flex flex-col gap-(--space-5)">
        <h2 id="guest-title" className={T.title}>
          {guest.title}
        </h2>
        <div className="flex flex-col gap-(--space-2)">
          <label htmlFor="word" className={T.rowTitle}>
            {guest.word}
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
            <p id="word-error" role="alert" className={T.attention}>
              {state.error}
            </p>
          )}
        </div>
        <div className="flex flex-col gap-(--space-3)">
          <button type="submit" name="intent" value="demo" className={CTA} aria-busy={pending}>
            {guest.demo}
          </button>
          <button type="submit" name="intent" value="new" className={CTA_QUIET} aria-busy={pending}>
            {guest.own}
          </button>
        </div>
      </form>
    </section>
  );
}
