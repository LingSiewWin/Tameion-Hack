"""Tameion's three account-page figures, in the Anatomy method: the object is the explanation."""
import math, os, sys
import isokit as k

OUT = sys.argv[1] if len(sys.argv) > 1 else "."
W, H = 600, 340
PLATE = dict(x=-125, y=-62, w=250, d=124, r=8)
PLATE_H, FOOT_H = 10, 5
TAU = 2 * math.pi


def base(P, strip=True, label=None):
    out = [f'<path class="h" d="{k.halo(PLATE, -PLATE_H - FOOT_H, PLATE_H, P, 4)}"/>']
    for x, y in k.by_depth(k.corners(PLATE, 20), P):
        out.append(k.solid(k.cylinder(x, y, 6, -PLATE_H - FOOT_H, 1.6, P, 14), "lo"))
        out.append(k.solid(k.cylinder(x, y, 4.8, -PLATE_H - FOOT_H + 1.6, FOOT_H - 1.6, P, 14), "mid"))
    out.append(k.solid(k.slab(PLATE, -PLATE_H, PLATE_H, P, 5, 1.4), "mid"))
    out.append(k.line(k.plan_outline(k.inset(PLATE, 5), 0, P, 5), "lo"))
    screws = k.corners(PLATE, 10)
    out.append(k.line("".join(k.ring(x, y, 2, 0, P, 12) for x, y in screws), "lo"))
    out.append(k.dots([k.iso((x, y, 0), P) for x, y in screws], 0.5))
    if strip:  # ruler strip along the near +x edge
        s = dict(x=108, y=-50, w=6, d=88, r=0.8)
        out.append(k.solid(k.slab(s, 0, 0.6, P, 2), "lo", crease="none"))
        minor, major = k.top_ticks(-46, 34, 4, 5, 108.8, 0.6, (2, 3.6), P, "y")
        out.append(k.line(minor, "lo") + k.line(major, "mid"))
        out.append(k.dots([k.iso((111, -48, 0.6), P), k.iso((111, 36, 0.6), P)], 0.45))
    if label:
        lp = label
        out.append(k.solid(k.slab(lp, 0, 0.8, P, 2), "mid"))
        out.append(k.line(k.plan_outline(k.inset(lp, 1.6), 0.8, P, 2), "ft"))
        x0, y0 = lp["x"] + 3, lp["y"] + 4
        rules = "".join(k.line_on_top((x0, y0 + i * 3), (x0 + L, y0 + i * 3), 0.8, P) for i, L in enumerate((18, 12, 22)))
        out.append(k.line(rules, "lo"))
        out.append(k.dots([k.iso((lp["x"] + 2, lp["y"] + 2, 0.8), P), k.iso((lp["x"] + lp["w"] - 2, lp["y"] + lp["d"] - 2, 0.8), P)], 0.45))
    return out


def phone_dock(P, cradle, phone):
    out = [k.solid(k.slab(cradle, 0, 3, P, 3), "lo")]
    out.append(k.solid(k.slab(phone, 3, 1.6, P, 3, 0.4), "hi", crease="lo"))
    out.append(k.line(k.plan_outline(k.inset(phone, 1.2), 4.6, P, 3), "ft"))
    out.append(k.dots([k.iso((phone["x"] + phone["w"] / 2, phone["y"] + 1.8, 4.6), P)], 0.45, "hi"))
    return out


def card_reader(P, block, z0, h, card_h=9):
    """A block with a slot on top and the agent's card standing in it, plus a green lamp."""
    out = [k.solid(k.slab(block, z0, h, P, 3), "mid")]
    cx = block["x"] + block["w"] / 2
    slot = dict(x=cx - 5, y=block["y"] + block["d"] / 2 - 1.1, w=10, d=2.2, r=1)
    out.append(k.line(k.plan_outline(slot, z0 + h, P, 2), "lo"))
    card = dict(x=cx - 4.5, y=slot["y"] + 0.6, w=9, d=1.0, r=0.3)
    out.append(k.solid(k.slab(card, z0 + h, card_h, P, 1, 0.4), "hi", crease="lo"))
    yf = card["y"] + card["d"] + 0.01
    out.append(k.line(k.segment((card["x"], yf, z0 + h + card_h * 0.5), (card["x"] + card["w"], yf, z0 + h + card_h * 0.5), P), "lo"))
    out.append(k.lamp(block["x"] + block["w"] - 3, block["y"] + 2.6, z0 + h, 1.1, P, "green"))
    return out


def dial(P, cx, cy, z, r, h, needle_turn, stop_turn=None, ticks=(40, 5)):
    top = z + h
    out = [k.solid(k.cylinder(cx, cy, r, z, h, P, 24, 0.8), "hi", crease="lo")]
    out.append(k.line(k.knurl(cx, cy, r, top - 3, top, 48, P, 0.3), "lo"))
    out.append(k.line(k.side_arc(cx, cy, r, top - 3, P, 16), "ft"))
    out.append(k.line(k.ring(cx, cy, r - 2.2, top, P, 24), "lo"))
    minor, major = k.radial_ticks(cx, cy, r - 3, ticks[0], ticks[1], top, (1.2, 2.4), P)
    out.append(k.line(minor, "lo") + k.line(major, "mid"))
    a = needle_turn * TAU
    out.append(k.line(k.line_on_top((cx - math.sin(a) * 2.8, cy + math.cos(a) * 2.8), (cx + math.sin(a) * (r - 4.2), cy - math.cos(a) * (r - 4.2)), top + 0.2, P), "hi", width=1.1))
    out.append(k.line(k.ring(cx, cy, 1.4, top, P, 12), "mid"))
    if stop_turn is not None:
        s = stop_turn * TAU
        out.append(k.solid(k.cylinder(cx + math.sin(s) * (r - 4.6), cy - math.cos(s) * (r - 4.6), 1.2, top, 2.2, P, 10), "hi", crease="lo"))
    return out


def knob(P, cx, cy, z, r, h, pointer=True, tone="mid"):
    out = [k.solid(k.cylinder(cx, cy, r, z, h, P, 20), tone)]
    out.append(k.line(k.knurl(cx, cy, r, z, z + h, int(r * 7), P), "lo"))
    if pointer:
        out.append(k.line(k.line_on_top((cx, cy), (cx + r + 0.8, cy), z + h, P), "hi"))
        out.append(k.dots([k.iso((cx, cy, z + h), P)], 0.5, "hi"))
    return out


def track(P, x0, w, y0, z, d=10, wall=4, floor=2):
    out = [k.solid(k.slab(dict(x=x0, y=y0, w=w, d=d, r=0.6), z, floor, P, 2), "mid")]
    for yy in (y0, y0 + d - 1.2):
        out.append(k.solid(k.slab(dict(x=x0, y=yy, w=w, d=1.2, r=0.3), z + floor, wall, P, 1), "mid", crease="none"))
    return out


def coin(P, cx, cy, z, r=3, lit=False):
    return k.solid(k.cylinder(cx, cy, r, z, 1.2, P, 18), "hi", crease="lo", lit=lit)


# ---------------------------------------------------------------------------
def hero():
    BOX = dict(x=-108, y=-48, w=50, d=46, r=3)
    BH = 44
    P = k.fit(k.box_corners(PLATE, -PLATE_H - FOOT_H, 0) + [(BOX["x"], BOX["y"], BH + 10), (BOX["x"] + BOX["w"], BOX["y"] + BOX["d"], BH + 10)], W, H, pad=22, az=46)
    body = base(P, label=dict(x=72, y=30, w=30, d=14, r=1.5))
    # the strongbox: the treasury
    yf = BOX["y"] + BOX["d"]
    body.append(k.solid(k.slab(BOX, 0, BH, P, 3, 0), "mid", inner=k.line(k.side_seam(BOX, 16, P, 3), "ft")))
    body.append(k.dots([k.iso((BOX["x"] + 3, yf + 0.01, zz), P) for zz in (40, 3)] + [k.iso((BOX["x"] + BOX["w"] - 3, yf + 0.01, zz), P) for zz in (40, 3)], 0.45))
    # sight glass with the level and the reserve floor
    body.append(k.line(k.face_rect(-70, -64, 8, 36, yf + 0.01, "y", P, 1.5), "mid"))
    body.append(k.line(k.segment((-70, yf + 0.02, 26), (-64, yf + 0.02, 26), P), "hi"))
    body.append(k.line(k.segment((-70, yf + 0.02, 13), (-64, yf + 0.02, 13), P), "mid"))
    # the lock: takes the phone's cable, nothing else
    esc = dict(x=-90, y=yf, w=12, d=1.4, r=1)
    body.append(k.solid(k.slab(esc, 20, 10, P, 2), "mid"))
    body.append(k.line(k.side_ring((-84, yf + 1.41, 25), 2.4, "y", P, 14), "lo"))
    body.append(k.line(k.segment((-84, yf + 1.42, 23.4), (-84, yf + 1.42, 26.6), P), "hi"))
    # lid and handle
    body.append(k.solid(k.slab(k.inset(BOX, -1), BH, 3.5, P, 3, 1), "mid", crease="lo"))
    for px in (-88, -72):
        body.append(k.solid(k.slab(dict(x=px, y=-27, w=3, d=3, r=1), BH + 3.5, 3, P, 1), "mid", crease="none"))
    body.append(k.solid(k.slab(dict(x=-89, y=-27.5, w=21, d=4, r=1.5), BH + 6.5, 2.5, P, 2, 0.4), "mid"))
    # the pay track leaving the box, with its gate and the coin in flight
    body += track(P, -58, 70, -16, 12)
    body.append(coin(P, -46, -11, 14))
    body.append(k.solid(k.slab(dict(x=-30, y=-16, w=1.6, d=10, r=0.3), 14, 7, P, 1), "hi", crease="lo"))
    body.append(k.solid(k.slab(dict(x=-30.7, y=-6.1, w=1.4, d=1.3, r=0.4), 15.8, 1.4, P, 1), "hi", crease="none"))
    body.append(k.solid(k.slab(dict(x=-33, y=-5, w=6, d=6, r=1), 0, 14, P, 2), "mid"))
    body.append(k.solid(k.cylinder(-30, -2, 2, 14, 2.5, P, 12), "mid"))
    body.append(coin(P, 4, -11, 14, lit=True))
    # the agent's card: read by the governor; the knob that set the cap drives the gate.
    # Wires lie on the plate, so they are drawn before the dial that overhangs them.
    body += card_reader(P, dict(x=-58, y=18, w=16, d=12, r=2), 0, 8)
    body += knob(P, -26, 42, 0, 4.5, 5)
    body.append(k.wire(k.iso((-42, 24, 4), P), k.iso((-4, 28.2, 1), P), via=[k.iso((-36, 24, 0), P), k.iso((-36, 32, 0), P), k.iso((-4, 32, 0), P)], tone="mid"))
    body.append(k.wire(k.iso((-26, 37.5, 1), P), k.iso((-30, 1.01, 4), P), via=[k.iso((-26, 8, 0), P), k.iso((-30, 8, 0), P)], tone="mid"))
    # the governor: the cap the owner set
    body.append(k.solid(k.cylinder(-4, 24, 5, 0, 2, P, 16), "lo"))
    body.append(k.solid(k.cylinder(-4, 24, 4, 2, 10, P, 16), "mid", crease="none"))
    body += dial(P, -4, 24, 12, 15, 6, 0.62, stop_turn=0.72)
    # the payee trays: a bill paid, one being paid, one due
    for xx in (20, 48):
        body.append(k.solid(k.slab(dict(x=xx, y=-50, w=3, d=92, r=0.8), 0, 2, P, 1), "mid"))
    for yy in (-53, 42):
        body.append(k.solid(k.slab(dict(x=18, y=yy, w=33, d=3, r=1), 0, 6, P, 1, 0.4), "mid"))
        body.append(k.dots([k.iso((22, yy + 1.5, 6), P), k.iso((47, yy + 1.5, 6), P)], 0.45))
    for i in range(3):
        tr = dict(x=23, y=-46 + 30 * i, w=22, d=24, r=2)
        body.append(k.solid(k.slab(tr, 2, 5, P, 3, 0.5), "mid"))
        body.append(k.line(k.plan_outline(k.inset(tr, 2.2), 7, P, 3), "ft"))
        if i == 0:
            body.append(coin(P, 34, -34, 7))
    # the owner's phone in its dock: the only key to the lock
    body += phone_dock(P, dict(x=-104, y=14, w=30, d=24, r=2), dict(x=-97, y=17, w=12, d=22, r=2))
    body.append(k.wire(k.iso((-91, 17, 3.6), P), k.iso((-84, yf + 1.41, 20.5), P), via=[k.iso((-91, 8, 0), P), k.iso((-84, 8, 0), P), k.iso((-84, yf + 1.41, 0), P)], tone="mid"))
    return k.figure(W, H, "A treasury that keeps itself solvent",
                    "A bench-top paying machine. A strongbox holds the company's money; a sight glass on its front shows the level and the reserve floor. "
                    "A track leaves the box through a gate and drops coins into a row of payee trays: one tray paid, one being paid now, one due. "
                    "The gate is driven from the agent's card reader, but only through a governor dial whose stop pin the owner set: the daily cap. "
                    "The owner's phone lies in a dock, cabled to the only lock on the box; nothing else opens it.",
                    body, paper=("var(--bg,#f6f5f1)", "var(--bg,#121311)"), name="hero")


# ---------------------------------------------------------------------------
def trust():
    CAB = dict(x=-110, y=-50, w=50, d=42, r=3)
    CH = 44
    P = k.fit(k.box_corners(PLATE, -PLATE_H - FOOT_H, 0) + [(CAB["x"], CAB["y"], CH + 12), (CAB["x"] + CAB["w"], CAB["y"] + CAB["d"], CH + 12)], W, H, pad=22, az=44)
    body = base(P, label=dict(x=72, y=34, w=30, d=14, r=1.5))
    yf = CAB["y"] + CAB["d"]
    # the cabinet holding the money and the payee rack
    body.append(k.solid(k.slab(CAB, 0, CH, P, 3, 0.8), "mid", inner=k.line(k.side_seam(CAB, 36, P, 3), "ft")))
    body.append(k.dots([k.iso((CAB["x"] + 3, yf + 0.01, zz), P) for zz in (40, 3)] + [k.iso((CAB["x"] + CAB["w"] - 3, yf + 0.01, zz), P) for zz in (40, 3)], 0.45))
    comb = dict(x=-106, y=-47, w=42, d=11, r=1.5)
    body.append(k.solid(k.slab(comb, CH, 4, P, 2), "mid"))
    for i in range(5):
        slot = dict(x=-103 + 8 * i, y=-43.5, w=5.5, d=4, r=0.8)
        body.append(k.line(k.plan_outline(slot, CH + 4, P, 2), "lo"))
        if i < 4:  # four approved payees, as tags in the rack
            tag = dict(x=-102.5 + 8 * i, y=-42.5, w=4.5, d=1.4, r=0.3)
            body.append(k.solid(k.slab(tag, CH + 4, 7, P, 1, 0.3), "hi", crease="lo"))
            body.append(k.line(k.segment((tag["x"], -41.09, CH + 7.5), (tag["x"] + 4.5, -41.09, CH + 7.5), P), "lo"))
    # the fifth slot: covered, keyed like the hatch
    cover = dict(x=-72.5, y=-47, w=9, d=11, r=1)
    body.append(k.solid(k.slab(cover, CH + 4, 1.5, P, 2, 0.3), "hi", crease="lo"))
    body.append(k.line(k.ring(-68, -41.5, 1.5, CH + 5.5, P, 12), "lo"))
    body.append(k.line(k.line_on_top((-68, -42.5), (-68, -40.5), CH + 5.52, P), "hi"))
    # the withdraw hatch, its hasp and lock
    hatch = dict(x=-100, y=yf, w=26, d=1.4, r=1)
    body.append(k.solid(k.slab(hatch, 6, 26, P, 2, 0.5), "mid", crease="lo"))
    body.append(k.solid(k.slab(dict(x=-80, y=yf + 1.4, w=5, d=1.6, r=0.5), 16, 6, P, 1), "mid"))
    body.append(k.line(k.side_ring((-77.5, yf + 3.01, 19), 2.2, "y", P, 14), "lo"))
    body.append(k.line(k.segment((-77.5, yf + 3.02, 17.6), (-77.5, yf + 3.02, 20.4), P), "hi"))
    body.append(k.lamp(-77.5, yf + 2.2, 22, 1.0, P, "red"))
    # the pay track: card in, plunger pushes the slide, as far as the stop
    body += track(P, -60, 90, -34, 8, d=14, wall=5)
    fl = dict(x=-60, y=-19, w=90, d=6, r=0.6)
    body.append(k.solid(k.slab(fl, 8, 1, P, 2), "lo", crease="none"))
    minor, major = k.top_ticks(-56, 26, 4, 5, -18.6, 9, (2, 3.6), P)
    body.append(k.line(minor, "lo") + k.line(major, "mid"))
    body.append(k.dots([k.iso((-58, -16, 9), P), k.iso((28, -16, 9), P)], 0.4))
    body += card_reader(P, dict(x=-59, y=-33, w=12, d=12, r=1), 10, 10, card_h=9)
    body.append(k.solid(k.slab(dict(x=-47, y=-27.6, w=11.2, d=1.2, r=0.5), 14.5, 1.2, P, 1), "hi", crease="none"))
    body.append(k.solid(k.slab(dict(x=-36, y=-32, w=16, d=10, r=1), 12, 3, P, 2, 0.5), "hi", crease="lo"))
    body.append(coin(P, -28, -27, 15, 3.2))
    body.append(k.solid(k.slab(dict(x=-8, y=-35, w=5, d=16, r=0.6), 10, 10, P, 1, 0.4), "hi", crease="lo", lit=True))
    body += knob(P, -5.5, -27, 20, 2, 3.5, pointer=False)
    body.append(k.solid(k.slab(dict(x=-3, y=-27.6, w=26.2, d=1.2, r=0.5), 14.5, 1.2, P, 1), "mid", crease="none"))
    body.append(k.solid(k.slab(dict(x=23, y=-35, w=3, d=16, r=0.5), 10, 9, P, 1), "mid"))
    body += knob(P, 28.5, -27, 12, 3, 4.5, pointer=True)
    # the owner's phone, cabled to the hatch lock
    body += phone_dock(P, dict(x=-116, y=12, w=28, d=22, r=2), dict(x=-110, y=15, w=11, d=20, r=1.8))
    body.append(k.wire(k.iso((-104.5, 15, 3.6), P), k.iso((-80, yf + 1.41, 19), P),
                       via=[k.iso((-104.5, 6, 0), P), k.iso((-86, 6, 0), P), k.iso((-86, yf + 1.41, 0), P), k.iso((-86, yf + 1.41, 19), P)], tone="mid"))
    # the delay: a timer drum with a ratchet; its wire reaches the cap screw; the lever's pawl cancels it.
    # The wire lies on the plate and runs behind the drum on screen, so it is drawn before the drum,
    # starting just outside the drum's front rim.
    body.append(k.wire(k.iso((50, 16.5, 0), P), k.iso((30.01, -27, 9), P), via=[k.iso((50, 24, 0), P), k.iso((36, 24, 0), P), k.iso((36, -27, 0), P)], tone="mid"))
    body += dial(P, 56, 0, 0, 15, 10, 0.18, ticks=(48, 6))
    body.append(k.line(k.knurl(56, 0, 15, 0.8, 6.4, 56, P, 0.3), "lo"))
    body += knob(P, 56, 0, 10, 4, 4)
    body.append(k.solid(k.cylinder(76, 15.2, 2, 0, 8, P, 12), "mid", crease="none"))
    body.append(k.solid(k.slab(dict(x=59, y=11.6, w=2.2, d=3.2, r=0.4), 8, 1.8, P, 1), "hi", crease="lo"))
    body.append(k.solid(k.slab(dict(x=60, y=14, w=18, d=2.4, r=1), 8, 1.8, P, 1, 0.3), "hi", crease="lo"))
    body.append(k.solid(k.cylinder(76, 15.2, 2.6, 9.8, 1.2, P, 12), "mid"))
    return k.figure(W, H, "Where the agent's key is not",
                    "A pay bench. The agent's card sits in a reader at the start of a track; its plunger pushes a coin slide along the track, but a stop block the owner set, "
                    "with a thumb screw, ends the travel: the cap. Behind it, a cabinet holds the money, and a rack on top holds four payee tags, with a fifth slot covered and locked. "
                    "The cabinet's hatch has a hasp, a lock and a red lamp: the card does nothing here. The owner's phone in its dock is cabled to that lock. "
                    "To the right, a timer drum with a ratchet and a winding knob is wired to the screw that moves the stop; a lever with a pawl on the drum cancels it before it runs down.",
                    body, name="trust")


# ---------------------------------------------------------------------------
def lazy():
    FP = dict(x=-26, y=-25, w=60, d=50, r=4)
    P = k.fit(k.box_corners(PLATE, -PLATE_H - FOOT_H, 0) + [(4, -52, 82), (1, -56, 78), (7, 6, 78)], W, H, pad=22, az=47)
    body = base(P, label=dict(x=72, y=36, w=30, d=14, r=1.5))
    # the wallet that already pays the bills
    WAL = dict(x=-112, y=-22, w=30, d=26, r=3)
    body.append(k.solid(k.slab(WAL, 0, 24, P, 3, 1), "mid", inner=k.line(k.side_seam(WAL, 18, P, 3), "ft")))
    body.append(k.line(k.plan_outline(dict(x=-104, y=-10, w=14, d=2.2, r=1), 24, P, 2), "lo"))
    body.append(k.dots([k.iso((WAL["x"] + 3, 4.01, zz), P) for zz in (21, 3)] + [k.iso((WAL["x"] + WAL["w"] - 3, 4.01, zz), P) for zz in (21, 3)], 0.45))
    # transfer: a chute from the wallet to the footprint, a coin rolling
    body += track(P, -82, 54, -14, 6, d=9, wall=3.5)
    body.append(coin(P, -52, -9.5, 8, 2.8))
    # allowance: a pipe with a tap the wallet controls
    body.append(k.solid(k.slab(dict(x=-82, y=4, w=52, d=3, r=1.4), 3, 3, P, 2), "mid", crease="none"))
    body.append(k.solid(k.cylinder(-58, 5.5, 3.6, 1, 8.5, P, 16), "mid"))
    body.append(k.solid(k.cylinder(-58, 5.5, 1, 9.5, 3, P, 10), "mid", crease="none"))
    body.append(k.solid(k.cylinder(-58, 5.5, 4.5, 12.5, 1.4, P, 20), "hi", crease="lo"))
    body.append(k.line(k.ring(-58, 5.5, 3.2, 13.9, P, 20), "lo"))
    body.append(k.line(k.line_on_top((-61.2, 5.5), (-54.8, 5.5), 13.92, P) + k.line_on_top((-58, 2.3), (-58, 8.7), 13.92, P), "hi"))
    body.append(k.solid(k.slab(dict(x=-30, y=2.5, w=4, d=6, r=0.8), 2, 6, P, 1), "mid"))
    # the reserved footprint: dotted, pegged, with its address stake; a soft shadow where the box will land
    body.append(f'<path class="h" style="filter:blur(5px)" d="{k.halo(dict(x=-14, y=-11, w=36, d=22, r=3), 0, 0.1, P, 3)}"/>')
    body.append(k.line(k.plan_outline(FP, 0.05, P, 4), "hi", dotted=True))
    for x, y in k.by_depth(k.corners(FP, 3), P):
        body.append(k.solid(k.cylinder(x, y, 1.4, 0, 3, P, 10), "hi", crease="lo"))
    body.append(k.solid(k.slab(dict(x=-32.8, y=-31.8, w=1.6, d=1.6, r=0.4), 0, 9, P, 1), "mid", crease="none"))
    body.append(k.solid(k.slab(dict(x=-36.5, y=-31.8, w=9, d=1.2, r=0.3), 5.5, 5, P, 1), "mid"))
    body.append(k.line(k.segment((-35.5, -30.59, 9.3), (-29.5, -30.59, 9.3), P) + k.segment((-35.5, -30.59, 7.6), (-31.5, -30.59, 7.6), P), "lo"))
    # the money that already arrived
    body.append(coin(P, -20, -9.5, 0, 2.8))
    body.append(k.solid(k.cylinder(-8, 12, 4.2, 0, 1.4, P, 18), "hi", crease="lo"))
    body.append(k.solid(k.cylinder(18, -12, 4.2, 0, 1.4, P, 18), "hi", crease="lo"))
    for zz in (0, 1.4, 2.8):
        body.append(k.solid(k.cylinder(14, 10, 5, zz, 1.4, P, 20), "hi", crease="lo"))
    # the crane: the first check, lowering the account onto its footprint
    body.append(k.solid(k.cylinder(4, -52, 7, 0, 2.5, P, 16), "lo"))
    body.append(k.solid(k.cylinder(4, -52, 4, 2.5, 75.5, P, 16), "mid", crease="none"))
    body.append(k.line("".join(k.side_arc(4, -52, 4, zz, P, 12) for zz in (22, 42)), "ft"))
    body.append(k.solid(k.cylinder(10.5, -52, 3, 56, 7, P, 14), "mid"))
    body.append(k.line(k.knurl(10.5, -52, 3, 56, 63, 18, P), "lo"))
    body.append(k.solid(k.cylinder(4, -52, 4.6, 78, 2, P, 16), "mid"))
    BOX = dict(x=-18, y=-15, w=44, d=30, r=3)
    body.append(k.solid(k.slab(BOX, 16, 28, P, 3), "hi", crease="lo", lit=True, inner=k.line(k.side_seam(BOX, 26, P, 3), "ft")))
    body.append(k.dots([k.iso((BOX["x"] + 3, 15.01, zz), P) for zz in (41, 19)] + [k.iso((BOX["x"] + BOX["w"] - 3, 15.01, zz), P) for zz in (41, 19)], 0.45))
    body.append(k.line(k.side_ring((4, 15.01, 30), 2.2, "y", P, 14), "lo"))
    body.append(k.line(k.segment((4, 15.02, 28.6), (4, 15.02, 31.4), P), "hi"))
    body.append(k.solid(k.slab(k.inset(BOX, -1), 44, 3, P, 3, 0.8), "hi", crease="lo", lit=True))
    body.append(k.solid(k.cylinder(4, 0, 2, 47, 3, P, 12), "mid"))
    body.append(k.solid(k.cylinder(4, 0, 0.9, 50, 20, P, 8), "mid", crease="none"))
    body.append(k.solid(k.slab(dict(x=0, y=-4, w=8, d=8, r=1), 70, 2, P, 2), "hi", crease="lo"))
    body.append(k.solid(k.slab(dict(x=1, y=-56, w=6, d=62, r=1), 72, 5, P, 2, 0.6), "mid"))
    body.append(k.dots([k.iso((4, yy, 77), P) for yy in (-50, -30, -10)], 0.45))
    return k.figure(W, H, "Money first, contract later",
                    "A plot on a bench. A dotted footprint with four corner pegs and an address stake marks where the account will stand: the address is reserved before anything is built. "
                    "Money already arrives there from the wallet that pays the bills today, two ways: coins rolling down a chute, and a pipe with a tap the wallet opens. "
                    "A stack of coins already sits inside the footprint. A crane behind it is lowering the strongbox, lid and lock, onto the pegs: the agent's first check builds the account around the money.",
                    body, name="lazy")


if __name__ == "__main__":
    os.makedirs(OUT, exist_ok=True)
    for name, fn in (("hero", hero), ("trust", trust), ("lazy", lazy)):
        svg = fn()
        with open(os.path.join(OUT, f"{name}.svg"), "w") as f:
            f.write(svg)
        print(name, len(svg.encode()), "bytes")
