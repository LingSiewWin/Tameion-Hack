#!/bin/zsh
# Regenerates assets/illustrations/*.svg and writes check renders (light, dark, 2x zooms, 390 px)
# to a temp folder with headless Chrome. Needs python3 and Google Chrome; nothing else.
set -e
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
D="$ROOT/assets/illustrations"
OUT="${TMPDIR:-/tmp}/tameion-illustrations"
mkdir -p "$OUT"
python3 "$HERE/figs.py" "$D"
CSS=':root { --bg:#f6f5f1; --ink:#171717; --mute:#6b6b66; --line:#dedcd4; --accent:#1f6f5c; --amber:#a3630a; --crimson:#9b2c2c; --card:#ffffff; }
:root[data-theme="dark"] { --bg:#121311; --ink:#ebe9e2; --mute:#9a9992; --line:#2b2c28; --accent:#5fc3a4; --amber:#e3a24a; --crimson:#e07070; --card:#1a1b18; }
body{margin:0;background:var(--bg);color:var(--ink);font:16px/1.5 -apple-system,system-ui,sans-serif}
main{max-width:720px;margin:0 auto;padding:24px 16px}
.card{background:var(--card);border:1px solid var(--line);border-radius:14px;padding:20px 22px;margin:0 0 16px}
.fig{overflow:hidden}@media (max-width:600px){.fig{margin:0 -16px}.fig svg{width:118%;margin-left:-9%}}'
for theme in light dark; do cat > "$OUT/check-$theme.html" <<EOF
<!doctype html><html lang="en" data-theme="$theme"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width"><style>$CSS</style></head><body><main>
<div class="fig">$(cat "$D/hero.svg")</div>
<div class="card"><div class="fig">$(cat "$D/trust.svg")</div></div>
<div class="card"><div class="fig">$(cat "$D/lazy.svg")</div></div>
</main></body></html>
EOF
done
for n in hero trust lazy; do cat > "$OUT/zoom-$n.html" <<EOF
<!doctype html><html lang="en" data-theme="light"><head><meta charset="utf-8"><style>$CSS body{background:var(--card)} svg{width:1400px}</style></head><body>$(cat "$D/$n.svg")</body></html>
EOF
done
CH="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
[ -x "$CH" ] || { echo "SVGs written to $D; no Chrome for the check renders"; exit 0; }
for theme in light dark; do "$CH" --headless=new --disable-gpu --hide-scrollbars --window-size=760,1500 --screenshot="$OUT/shot-$theme.png" "file://$OUT/check-$theme.html" 2>/dev/null; done
for n in hero trust lazy; do "$CH" --headless=new --disable-gpu --hide-scrollbars --window-size=1400,800 --screenshot="$OUT/zoom-$n.png" "file://$OUT/zoom-$n.html" 2>/dev/null; done
"$CH" --headless=new --disable-gpu --hide-scrollbars --window-size=390,1200 --screenshot="$OUT/phone-light.png" "file://$OUT/check-light.html" 2>/dev/null
echo "check renders in $OUT"; ls "$OUT"/*.png
