#!/usr/bin/env python3
"""A copy rehearsal, not validation (docs/63, docs/64): five hand-written
buyer personas read the exact words of the account page's Start view and say
what they think it is, which words lost them, and what would stop them.
Runs on local models through Ollama (free; nothing leaves the machine).

  python3 scripts/research/persona_rehearsal.py page.txt out.json [model ...]

page.txt is the visible text of `/` (the rehearsal reads what a buyer reads).
A word counts as a problem only if at least 3 of 5 personas name it on every
model run -- the rule docs/63 set before any answer was seen."""
import json, sys, urllib.request
from collections import Counter

PERSONAS = [
    ("DAO treasurer", "You run the treasury of a 40-person DAO. You pay ~25 contributors and 6 vendors in USDC every month from a Safe. You were burned once by a multisig signer who approved a bad transaction. You read everything twice."),
    ("Startup finance lead", "You are the finance lead at a 12-person onchain startup. You came from a fintech. You use QuickBooks and a bank; crypto payments are a side process you dislike. You want fewer manual steps, not new jargon."),
    ("Founder acting as CFO", "You are a technical founder of a 5-person team who also does the finance. You pay hosting, an auditor, two contractors. You have little time and you skim."),
    ("Outsourced controller", "You are an accountant who keeps the books for six small crypto companies. You need records you can reconcile and hand to auditors. You never hold client keys."),
    ("Ops lead burned by an incident", "You run operations at a protocol whose hot wallet was drained by a leaked API key last year. You distrust anything that holds a key. Your first question is always: what can this thing do if it is compromised?"),
]

QUESTIONS = (
    "Answer as this person, honestly, in JSON with exactly these keys:\n"
    '"what_it_is": one sentence in your own words,\n'
    '"confusing_words": a list of the exact words or phrases from the page you did not understand or did not trust (copy them exactly),\n'
    '"stopper": the one thing that would stop you trying it this week,\n'
    '"trust": a number 1-5 for how much you would trust it with real money after reading only this,\n'
    '"first_question": the first question you would ask the founder.'
)


def ask(model, persona, page):
    body = {
        "model": model,
        "format": "json",
        "stream": False,
        "options": {"temperature": 0.4},
        "messages": [
            {"role": "system", "content": f"You are role-playing a real buyer. {persona} Never break character. Do not be polite for its own sake."},
            {"role": "user", "content": f"This is the page you just opened:\n\n---\n{page}\n---\n\n{QUESTIONS}"},
        ],
    }
    req = urllib.request.Request("http://127.0.0.1:11434/api/chat", data=json.dumps(body).encode(),
                                 headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=600) as r:
        content = json.loads(r.read())["message"]["content"]
    try:
        return json.loads(content)
    except json.JSONDecodeError:
        return {"raw": content}


def main():
    page = open(sys.argv[1]).read()
    out = sys.argv[2]
    models = sys.argv[3:] or ["llama3.1:8b", "mistral:7b"]
    runs = []
    for m in models:
        for name, persona in PERSONAS:
            a = ask(m, persona, page)
            runs.append({"model": m, "persona": name, "answer": a})
            print(f"{m} / {name}: trust {a.get('trust')} — {str(a.get('what_it_is'))[:90]}", flush=True)
    tally = {}
    for m in models:
        c = Counter()
        for r in runs:
            if r["model"] != m:
                continue
            words = r["answer"].get("confusing_words") or []
            for w in {str(x).strip().lower() for x in words if str(x).strip()}:
                c[w] += 1
        tally[m] = c
    # a word is a problem only if >= 3 of 5 personas named it on every model
    common = set.intersection(*[{w for w, n in c.items() if n >= 3} for c in tally.values()]) if tally else set()
    json.dump({"models": models, "runs": runs,
               "tally": {m: c.most_common(25) for m, c in tally.items()},
               "problem_words": sorted(common)}, open(out, "w"), indent=2)
    print("problem words (>=3/5 on every model):", sorted(common))


if __name__ == "__main__":
    main()
