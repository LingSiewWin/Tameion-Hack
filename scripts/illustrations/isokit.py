"""A small isometric SVG kit, written for Tameion's illustrations.

Re-implements the method of the Anatomy skill (camera, prisms extruded from a
floor plan, four side tones by facing, lighter top, painter's order, 0.6 px
lines, screws as dots, terminal-dotted wires) in plain Python so the figures
can be generated without running anything from that repository.
"""
import math

R = math.pi / 180
UNIT = math.sqrt(1.6)
SHADES = 4


class Proj:
    def __init__(self, origin, scale, az=45, el=30):
        self.origin, self.scale, self.az, self.el = origin, scale, az, el
        a, e = az * R, el * R
        self.sinA, self.cosA = math.sin(a), math.cos(a)
        self.sinE, self.cosE = math.sin(e), math.cos(e)
        self.k = scale * UNIT


def iso(p, P):
    x, y, z = p
    return (P.origin[0] + (x * P.sinA - y * P.cosA) * P.k,
            P.origin[1] + ((x * P.cosA + y * P.sinA) * P.sinE - z * P.cosE) * P.k)


def depth(p, P):
    x, y, z = p
    return (x * P.cosA + y * P.sinA) * P.cosE + z * P.sinE


def toward(P):
    return (P.cosA, P.sinA)


def t(v):
    s = f"{round(v * 10) / 10:.1f}".rstrip("0").rstrip(".")
    return "0" if s in ("", "-0") else s


def path(points, closed=False):
    """Relative path data on a tenth grid: M, then h/v/l deltas (no drift: deltas of rounded absolutes)."""
    if not points:
        return ""
    out, px, py = [], None, None
    for x, y in points:
        rx, ry = round(x * 10) / 10, round(y * 10) / 10
        if px is None:
            out.append(f"M{t(rx)} {t(ry)}")
        else:
            dx, dy = round((rx - px) * 10) / 10, round((ry - py) * 10) / 10
            if dy == 0:
                out.append(f"h{t(dx)}")
            elif dx == 0:
                out.append(f"v{t(dy)}")
            else:
                out.append(f"l{t(dx)} {t(dy)}")
        px, py = rx, ry
    return "".join(out) + ("Z" if closed else "")


def rounded_plan(plan, steps=3):
    x, y, w, d, r = plan["x"], plan["y"], plan["w"], plan["d"], plan.get("r", 0)
    radius = max(0, min(r, w / 2, d / 2))
    corners = [(x + w - radius, y + radius, -90), (x + w - radius, y + d - radius, 0),
               (x + radius, y + d - radius, 90), (x + radius, y + radius, 180)]
    out = []
    for cx, cy, start in corners:
        count = steps if radius > 0 else 0
        for s in range(count + 1):
            ang = (start + (90 * s / count if count else 45)) * R
            out.append((cx + radius * math.cos(ang), cy + radius * math.sin(ang)))
    return out


def circle_ring(cx, cy, r, steps=16):
    return [(cx + r * math.cos(2 * math.pi * s / steps), cy + r * math.sin(2 * math.pi * s / steps)) for s in range(steps)]


def signed_area(pts):
    a = 0
    for i in range(len(pts)):
        ax, ay = pts[i]
        bx, by = pts[(i + 1) % len(pts)]
        a += ax * by - bx * ay
    return a / 2


def oriented(pts):
    return list(reversed(pts)) if signed_area(pts) < 0 else list(pts)


def convex_hull(points):
    pts = sorted(set(points))
    if len(pts) < 3:
        return pts
    cross = lambda o, a, b: (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    lower = []
    for p in pts:
        while len(lower) >= 2 and cross(lower[-2], lower[-1], p) <= 0:
            lower.pop()
        lower.append(p)
    upper = []
    for p in reversed(pts):
        while len(upper) >= 2 and cross(upper[-2], upper[-1], p) <= 0:
            upper.pop()
        upper.append(p)
    return lower[:-1] + upper[:-1]


def runs_of(flags, want):
    n = len(flags)
    start = next((i for i in range(n) if flags[i] != flags[(i - 1) % n]), -1)
    if start < 0:
        return [list(range(n)) + [0]] if flags[0] == want else []
    runs, run = [], None
    for s in range(n):
        i = (start + s) % n
        if flags[i] == want:
            if run is None:
                run = [i]
            run.append((i + 1) % n)
        elif run:
            runs.append(run)
            run = None
    if run:
        runs.append(run)
    return runs


def extrude(ring, z, h, P, bevel=0):
    n = len(ring)
    counter = signed_area(ring) > 0
    vx, vy = toward(P)
    normals = []
    for i in range(n):
        ax, ay = ring[i]
        bx, by = ring[(i + 1) % n]
        dx, dy = bx - ax, by - ay
        L = math.hypot(dx, dy) or 1
        normals.append((dy / L, -dx / L) if counter else (-dy / L, dx / L))
    facing = [nx * vx + ny * vy > 1e-9 for nx, ny in normals]
    top = [iso((x, y, z + h), P) for x, y in ring]
    bot = [iso((x, y, z), P) for x, y in ring]
    outline, crease = [], []
    for run in runs_of(facing, False):
        outline.append(path([top[i] for i in run]))
    for run in runs_of(facing, True):
        crease.append(path([top[i] for i in run]))
        if h > 0:
            outline.append(path([bot[i] for i in run]))
    if h > 0:
        for i in range(n):
            if facing[(i - 1) % n] != facing[i]:
                outline.append(path([top[i], bot[i]]))
    fill = path(convex_hull(top + bot), True)
    buckets = ["", "", "", ""]
    if h > 0:
        lx, ly = -P.sinA, P.cosA
        for run in runs_of(facing, True):
            edges = run[:-1]
            shade_of = lambda e: min(SHADES - 1, max(0, int(((normals[e][0] * lx + normals[e][1] * ly + 1) / 2) * SHADES)))
            a = 0
            while a < len(edges):
                b = a
                while b + 1 < len(edges) and shade_of(edges[b + 1]) == shade_of(edges[a]):
                    b += 1
                vs = run[a:b + 2]
                poly = [top[i] for i in vs] + [bot[i] for i in reversed(vs)]
                buckets[shade_of(edges[a])] += path(poly, True)
                a = b + 1
    bev = ""
    if bevel and bevel > 0:
        inset = []
        for i, (x, y) in enumerate(ring):
            ax, ay = normals[(i - 1) % n]
            bx, by = normals[i]
            mx, my = ax + bx, ay + by
            L = math.hypot(mx, my) or 1
            inset.append((x - mx / L * bevel, y - my / L * bevel))
        itop = [iso((x, y, z + h), P) for x, y in inset]
        for run in runs_of(facing, True):
            bev += path([top[run[0]]] + [itop[i] for i in run[1:-1]] + [top[run[-1]]])
    return dict(fill=fill, outline="".join(outline), crease="".join(crease), top=path(oriented(top), True), shades=buckets, bevel=bev)


def slab(plan, z, h, P, steps=3, bevel=0):
    return extrude(rounded_plan(plan, steps), z, h, P, bevel)


def cylinder(cx, cy, r, z, h, P, steps=16, bevel=0):
    return extrude(circle_ring(cx, cy, r, steps), z, h, P, bevel)


def inset(plan, by):
    return dict(x=plan["x"] + by, y=plan["y"] + by, w=plan["w"] - 2 * by, d=plan["d"] - 2 * by, r=max(0, plan.get("r", 0) - by))


def on_top(points, z, P, closed=False):
    return path([iso((x, y, z), P) for x, y in points], closed)


def plan_outline(plan, z, P, steps=3):
    return on_top(rounded_plan(plan, steps), z, P, True)


def line_on_top(a, b, z, P):
    return on_top([a, b], z, P)


def segment(a, b, P):
    return path([iso(a, P), iso(b, P)])


def rise(at, z0, z1, P):
    return segment((at[0], at[1], z0), (at[0], at[1], z1), P)


def ring(cx, cy, r, z, P, steps=16):
    return on_top(circle_ring(cx, cy, r, steps), z, P, True)


def corners(plan, by=0):
    x, y, w, d = plan["x"], plan["y"], plan["w"], plan["d"]
    return [(x + by, y + by), (x + w - by, y + by), (x + by, y + d - by), (x + w - by, y + d - by)]


def box_corners(plan, z0, z1):
    x, y, w, d = plan["x"], plan["y"], plan["w"], plan["d"]
    return [(px, py, z) for px in (x, x + w) for py in (y, y + d) for z in (z0, z1)]


def by_depth(points, P):
    return sorted(points, key=lambda p: depth((p[0], p[1], 0), P))


def knurl(cx, cy, r, z0, z1, count, P, ins=0.4):
    vx, vy = toward(P)
    out = ""
    for i in range(count):
        ang = 2 * math.pi * i / count
        nx, ny = math.cos(ang), math.sin(ang)
        if nx * vx + ny * vy < 0.15:
            continue
        out += segment((cx + r * nx, cy + r * ny, z0 + ins), (cx + r * nx, cy + r * ny, z1 - ins), P)
    return out


def side_arc(cx, cy, r, z, P, steps=16):
    vx, vy = toward(P)
    f = math.atan2(vy, vx)
    pts = []
    for s in range(steps + 1):
        ang = f - math.pi / 2 + math.pi * s / steps
        pts.append(iso((cx + r * math.cos(ang), cy + r * math.sin(ang), z), P))
    return path(pts)


def ring_seam(rg, z, P):
    n = len(rg)
    counter = signed_area(rg) > 0
    vx, vy = toward(P)
    facing = []
    for i in range(n):
        ax, ay = rg[i]
        bx, by = rg[(i + 1) % n]
        dx, dy = bx - ax, by - ay
        nx, ny = (dy, -dx) if counter else (-dy, dx)
        facing.append(nx * vx + ny * vy > 1e-9)
    return "".join(path([iso((rg[i][0], rg[i][1], z), P) for i in run]) for run in runs_of(facing, True))


def side_seam(plan, z, P, steps=3):
    return ring_seam(rounded_plan(plan, steps), z, P)


def side_ring(center, r, axis, P, steps=16):
    px, py, pz = center
    pts = []
    for s in range(steps):
        ang = 2 * math.pi * s / steps
        u, v = r * math.cos(ang), r * math.sin(ang)
        pts.append(iso((px, py + u, pz + v) if axis == "x" else (px + u, py, pz + v), P))
    return path(pts, True)


def face_rect(a0, a1, z0, z1, at, axis, P, r=0, steps=2):
    """A rounded rectangle on a vertical face: axis 'y' = face y=at (varies x), 'x' = face x=at (varies y)."""
    plan = dict(x=a0, y=z0, w=a1 - a0, d=z1 - z0, r=r)
    pts = rounded_plan(plan, steps) if r else [(a0, z0), (a1, z0), (a1, z1), (a0, z1)]
    return path([iso((u, at, zz) if axis == "y" else (at, u, zz), P) for u, zz in pts], True)


def top_ticks(frm, to, step, every, edge, z, lengths, P, axis="x"):
    minor, major = "", ""
    first = math.ceil(frm / step) * step
    at, idx = first, round(first / step)
    while at <= to + 1e-9:
        big = idx % every == 0
        L = lengths[1] if big else lengths[0]
        a = (at, edge) if axis == "x" else (edge, at)
        b = (at, edge + L) if axis == "x" else (edge + L, at)
        if big:
            major += line_on_top(a, b, z, P)
        else:
            minor += line_on_top(a, b, z, P)
        at += step
        idx += 1
    return minor, major


def radial_ticks(cx, cy, r, count, every, z, lengths, P, frm=0.0, span=2 * math.pi):
    minor, major = "", ""
    closed = abs(span - 2 * math.pi) < 1e-9
    steps = count if closed else count + 1
    for i in range(steps):
        ang = frm + span * i / count
        big = i % every == 0
        inner = r - (lengths[1] if big else lengths[0])
        sx, sy = math.sin(ang), -math.cos(ang)
        d = line_on_top((cx + sx * inner, cy + sy * inner), (cx + sx * r, cy + sy * r), z, P)
        if big:
            major += d
        else:
            minor += d
    return minor, major


def halo(plan, z, h, P, steps=3):
    return slab(plan, z, h, P, steps)["fill"]


def fit(points, w, h, pad=24, az=45, el=30, headroom=0):
    unit = Proj((0, 0), 1, az, el)
    flat = [iso(p, unit) for p in points]
    xs, ys = [p[0] for p in flat], [p[1] for p in flat]
    left, right, up, down = min(xs), max(xs), min(ys), max(ys)
    scale = min((w - 2 * pad) / (right - left), (h - 2 * pad - headroom) / (down - up))
    origin = (w / 2 - (left + right) / 2 * scale, headroom + (h - headroom) / 2 - (up + down) / 2 * scale)
    return Proj(origin, scale, az, el)


# ---- SVG strings -------------------------------------------------------------

def solid(paths, tone="hi", crease="ft", lit=False, flat=False, inner="", cls=""):
    # ponytail: no paper fill under the faces; the faces cover the convex hull exactly and
    # a same-colour hairline stroke on each face closes the anti-aliasing seams (the kit's seamless mode).
    parts = [f'<path class="f" d="{paths["fill"]}"/>'] if flat else []
    if not flat:
        for i, d in enumerate(paths["shades"]):
            if d:
                parts.append(f'<path class="s s{i}" d="{d}"/>')
        parts.append(f'<path class="tp" d="{paths["top"]}"/>')
    if crease != "none" and paths["crease"]:
        parts.append(f'<path class="l {crease}" d="{paths["crease"]}"/>')
    if crease != "none" and paths.get("bevel"):
        parts.append(f'<path class="l {crease}" d="{paths["bevel"]}"/>')
    if inner:
        parts.append(inner)
    parts.append(f'<path class="l e {tone}" d="{paths["outline"]}"/>')
    attrs = ' class="o lit"' if lit else ' class="o"'
    if cls:
        attrs = f' class="o {cls}{" lit" if lit else ""}"'
    return f"<g{attrs}>{''.join(parts)}</g>"


def line(d, tone="lo", dotted=False, dashed=False, cls="", width=None):
    if not d:
        return ""
    c = f"l {tone}" + (" dt" if dotted else "") + (" ds" if dashed else "") + (f" {cls}" if cls else "")
    st = f' style="stroke-width:{width}px"' if width else ""
    return f'<path class="{c}" d="{d}"{st}/>'


def dots(points, size=0.5, tone="mid"):
    return f'<g class="d {tone}">' + "".join(f'<circle cx="{t(x)}" cy="{t(y)}" r="{size}"/>' for x, y in points) + "</g>"


def wire(frm, to, via=(), tone="lo", ends="both", size=1.2, dashed=False):
    d = path([frm, *via, to])
    end = lambda p: f'<circle class="we" cx="{t(p[0])}" cy="{t(p[1])}" r="{size}"/>'
    return (f'<g class="w {tone}">{line(d, tone, dashed=dashed)}'
            f'{end(frm) if ends in ("both", "from") else ""}{end(to) if ends in ("both", "to") else ""}</g>')


def lamp(cx, cy, z, r, P, colour):
    """A tiny indicator lamp: a short cylinder whose top takes a colour token."""
    body = cylinder(cx, cy, r, z, 1.2, P, 12)
    return (f'<g class="o">'
            + "".join(f'<path class="s s{i}" d="{d}"/>' for i, d in enumerate(body["shades"]) if d)
            + f'<path class="lampt {colour}" d="{body["top"]}"/>'
            + f'<path class="l e mid" d="{body["outline"]}"/></g>')


def css(paper_light, paper_dark):
    light = (f"--p:{paper_light};--face:#fdfcfa;--top:#fdfcfa;--s0:#d6d4cb;--s1:#dfddd5;--s2:#e8e6df;--s3:#f0eee8;"
             "--lt:#ffffff;--ls:#eeece6;--hi:#2a2a28;--mid:#86857f;--lo:#b5b3ab;--faint:var(--line,#dedcd4);"
             "--lit:var(--ink,#171717);--halo:#8a8878;--ho:.3;--dot:#3a3a38;--red:var(--crimson,#9b2c2c);--green:var(--accent,#1f6f5c)")
    dark = (f"--p:{paper_dark};--face:#1f201d;--top:#222320;--s0:#0c0d0b;--s1:#0f100e;--s2:#121311;--s3:#161715;"
            "--lt:#2d2e2a;--ls:#242522;--hi:#d6d4cc;--mid:#6f6f69;--lo:#44453f;--faint:var(--line,#2b2c28);"
            "--lit:var(--ink,#ebe9e2);--halo:#050504;--ho:.9;--dot:#e6e4dc;--red:var(--crimson,#e07070);--green:var(--accent,#5fc3a4)")
    return (
        f".tm-fig{{{light};--w:.6px;display:block;width:100%;height:auto;overflow:visible}}"
        f"@media (prefers-color-scheme:dark){{:root:not([data-theme=light]) .tm-fig,:root:not([data-theme=light]).tm-fig{{{dark}}}}}"
        f":root[data-theme=dark] .tm-fig,:root[data-theme=dark].tm-fig{{{dark}}}"
        ".tm-fig path,.tm-fig circle{vector-effect:non-scaling-stroke}"
        ".tm-fig .f{fill:var(--p);stroke:none}.tm-fig .lit>.f{fill:var(--face)}"
        ".tm-fig .s,.tm-fig .tp{stroke-width:.5px;stroke-linejoin:round}"
        ".tm-fig .s0{fill:var(--s0);stroke:var(--s0)}.tm-fig .s1{fill:var(--s1);stroke:var(--s1)}.tm-fig .s2{fill:var(--s2);stroke:var(--s2)}.tm-fig .s3{fill:var(--s3);stroke:var(--s3)}"
        ".tm-fig .tp{fill:var(--top);stroke:var(--top)}.tm-fig .lit>.tp{fill:var(--lt);stroke:var(--lt)}.tm-fig .lit>.s{fill:var(--ls);stroke:var(--ls)}.tm-fig .lit>.e{stroke:var(--lit)}"
        ".tm-fig .h{fill:var(--halo);stroke:none;filter:blur(9px);opacity:var(--ho);transform:translateY(3px)}"
        ".tm-fig .l{fill:none;stroke:var(--lo);stroke-width:var(--w);stroke-linecap:round;stroke-linejoin:round}"
        ".tm-fig .l.hi{stroke:var(--hi)}.tm-fig .l.mid{stroke:var(--mid)}.tm-fig .l.lo{stroke:var(--lo)}.tm-fig .l.ft{stroke:var(--faint)}"
        ".tm-fig .l.lit{stroke:var(--lit)}.tm-fig .l.red{stroke:var(--red)}.tm-fig .l.green{stroke:var(--green)}"
        ".tm-fig .l.dt{stroke-dasharray:.01 3.5;stroke-width:1}.tm-fig .l.ds{stroke-dasharray:3 3}"
        ".tm-fig .we{fill:var(--p);stroke:var(--lo);stroke-width:var(--w)}.tm-fig .w.mid .we{stroke:var(--mid)}.tm-fig .w.hi .we{stroke:var(--hi)}"
        ".tm-fig .d circle{fill:var(--dot);stroke:none;opacity:.32}.tm-fig .d.hi circle{opacity:.85}.tm-fig .d.mid circle{opacity:.45}.tm-fig .d.lo circle{opacity:.22}"
        ".tm-fig .lampt{stroke:none}.tm-fig .lampt.red{fill:var(--red)}.tm-fig .lampt.green{fill:var(--green)}"
    )


def figure(w, h, title, desc, body, paper=("var(--card,#ffffff)", "var(--card,#1a1b18)"), name="fig"):
    tid, did = f"{name}-t", f"{name}-d"
    return (f'<svg xmlns="http://www.w3.org/2000/svg" class="tm-fig" viewBox="0 0 {w} {h}" role="img" aria-labelledby="{tid} {did}">'
            f"<title id=\"{tid}\">{title}</title><desc id=\"{did}\">{desc}</desc>"
            f"<style>{css(*paper)}</style>{''.join(body)}</svg>")
