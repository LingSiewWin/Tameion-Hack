// Tami, the 120-second demo. Scenes and timings: the founder's brief (docs: V1 seat, 2026-10-18).
// Every figure, hash and log line comes from numbers.json (scripts/video/numbers.mjs, read from proof/);
// captions and voice from cues.json (scripts/video/voice.mjs); footage boxes and clip lengths from footage.json
// (scripts/video/record.mjs, recorded from the live page). Nothing on screen is typed here by hand except the
// founder's own sentences.
import React from "react";
import { AbsoluteFill, Audio, OffthreadVideo, Sequence, interpolate, spring, staticFile, useCurrentFrame, useVideoConfig } from "remotion";
import { TransitionSeries, linearTiming } from "@remotion/transitions";
import { fade } from "@remotion/transitions/fade";
import { loadFont as loadSans } from "@remotion/google-fonts/IBMPlexSans";
import { loadFont as loadMono } from "@remotion/google-fonts/IBMPlexMono";
import N from "./numbers.json";
import CUES from "./cues.json";
import F from "./footage.json";

const SANS = loadSans("normal", { weights: ["400", "500", "600"], subsets: ["latin"] }).fontFamily;
const MONO = loadMono("normal", { weights: ["400", "500"], subsets: ["latin"] }).fontFamily;

// apps/home/DESIGN.md: light (Owner, land) and dark (Agent, sea).
const C = {
  paper: "#EEF2EA", ink: "#000000", ink2: "#5B625B", rule: "#D5DCD2",
  sea: "#0E1B22", seaInk: "#EEF2EA", seaInk2: "#A9B4AD", seaRule: "#2A3C45",
  attention: "#B3261E", transit: "#3FB8B0",
};
const FADE = 15; // the one crossfade per scene, in frames
const SCENES = [210, 330, 600, 660, 540, 540, 420, 300]; // 0-7, 7-18, 18-38, 38-60, 60-78, 78-96, 96-110, 110-120 s
const PAD = 120; // left margin

const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;
const typed = (text: string, frame: number, start: number, perFrame: number) =>
  text.slice(0, Math.max(0, Math.floor((frame - start) * perFrame)));
const fadeIn = (frame: number, start: number, len = 10) => interpolate(frame, [start, start + len], [0, 1], clamp);

/** A press: rises 12 px and fades in on a spring (damping 200). */
const Press: React.FC<{ at: number; children: React.ReactNode; style?: React.CSSProperties }> = ({ at, children, style }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const p = spring({ frame: frame - at, fps, config: { damping: 200 } });
  return <div style={{ opacity: p, transform: `translateY(${(1 - p) * 12}px)`, ...style }}>{children}</div>;
};

const Source: React.FC<{ children: React.ReactNode; dark?: boolean; at?: number }> = ({ children, dark, at = 0 }) => {
  const frame = useCurrentFrame();
  return (
    <div style={{ fontFamily: MONO, fontSize: 36, color: dark ? C.seaInk2 : C.ink2, opacity: fadeIn(frame, at) }}>{children}</div>
  );
};

// 0-7 s: the name and the line.
const Title: React.FC = () => (
  <AbsoluteFill style={{ background: C.paper, padding: `0 ${PAD + 40}px`, justifyContent: "center" }}>
    <Press at={6}>
      <div style={{ fontFamily: SANS, fontWeight: 600, fontSize: 120, color: C.ink, lineHeight: 1.1 }}>Tami</div>
    </Press>
    <Press at={30}>
      <div style={{ fontFamily: SANS, fontSize: 56, color: C.ink, marginTop: 28 }}>The account an agent operator gives its agents.</div>
    </Press>
  </AbsoluteFill>
);

// 7-18 s: the organiser's sentence, typed.
const Organiser: React.FC = () => {
  const frame = useCurrentFrame();
  const s = N.organiser.sentence.text;
  return (
    <AbsoluteFill style={{ background: C.paper, padding: `150px ${PAD + 40}px` }}>
      <Source at={4}>{N.organiser.file}:{N.organiser.sentence.lines}</Source>
      <div style={{ fontFamily: SANS, fontSize: 40, color: C.ink2, marginTop: 64, opacity: fadeIn(frame, 10) }}>The organiser's brief</div>
      <div style={{ fontFamily: SANS, fontSize: 64, lineHeight: 1.3, color: C.ink, marginTop: 20, maxWidth: 1500 }}>
        {typed(s, frame, 20, s.length / 150)}
      </div>
    </AbsoluteFill>
  );
};

// 18-38 s: the three tests, their real log lines (hashes elided outside the agent view), a tick each.
const KEY: Record<string, string[]> = {
  wrong_vendor: ['refused "payee_not_approved" (executed: "no")', "unapproved balance 0 wei"],
  phantom_invoice: ["phantom 422", "phantom in it: no"],
  double_pay_on_retry: ["Paid events for it: 1"],
};
const emphasise = (shown: string, keys: string[]) => {
  const parts: React.ReactNode[] = [];
  let rest = shown, k = 0;
  while (rest.length) {
    const hits = keys.map((key) => [rest.indexOf(key), key] as const).filter(([i]) => i >= 0).sort((a, b) => a[0] - b[0]);
    if (!hits.length) { parts.push(rest); break; }
    const [i, key] = hits[0];
    parts.push(rest.slice(0, i), <span key={k++} style={{ color: C.seaInk, fontWeight: 500 }}>{key}</span>);
    rest = rest.slice(i + key.length);
  }
  return parts;
};
const TEST_AT = [75, 240, 396]; // frames into the scene; each lands with its caption
const Tests: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const cmd = N.organiser.command.text;
  return (
    <AbsoluteFill style={{ background: C.sea, padding: "64px 64px", fontFamily: MONO, color: C.seaInk2 }}>
      <Source dark>{N.organiser.file}, run {N.organiser.run.text.replace(/^date: /, "")}</Source>
      <Source dark>hashes and addresses are shown here as 0x…; the file has them in full</Source>
      <div style={{ fontSize: 36, marginTop: 28, color: C.seaInk, whiteSpace: "nowrap" }}>{typed(cmd, frame, 10, 2.6)}</div>
      {N.organiser.tests.map((t, i) => {
        const at = TEST_AT[i];
        if (frame < at) return null;
        const okAt = at + 6 + Math.ceil(t.shown.length / 5) + 8;
        const done = frame >= okAt;
        const tick = spring({ frame: frame - okAt, fps, config: { damping: 200 } });
        // A finished test folds to its name when the next one starts; the last stays open.
        const fold = i < TEST_AT.length - 1 ? interpolate(frame, [TEST_AT[i + 1] - 12, TEST_AT[i + 1]], [1, 0], clamp) : 1;
        return (
          <div key={t.name} style={{ marginTop: 30, display: "flex", gap: 24 }}>
            <div style={{ width: 40, fontSize: 40, color: C.seaInk, opacity: tick, transform: `scale(${0.6 + 0.4 * tick})` }}>✓</div>
            <div style={{ flex: 1 }}>
              <div style={{ fontSize: 40, color: C.seaInk, opacity: fadeIn(frame, at, 6) }}>
                test {t.name} ...{done ? <span style={{ color: C.seaInk2 }}> {t.ok}</span> : null}
              </div>
              <div style={{ fontSize: 36, lineHeight: 1.32, marginTop: 6 * fold, maxHeight: 260 * fold, opacity: fold, overflow: "hidden", overflowWrap: "anywhere" }}>
                {done ? emphasise(t.shown, KEY[t.name]) : typed(t.shown, frame, at + 6, 5)}
              </div>
            </div>
          </div>
        );
      })}
      <div style={{ fontSize: 36, marginTop: 30, color: C.seaInk, opacity: fadeIn(frame, 500) }}>{N.organiser.result.text}</div>
    </AbsoluteFill>
  );
};

// 38-60 s: the owner view, recorded live; callouts beside it during the hold at the top.
const OX = 72, OY = 64, FW = F.size.w, FH = F.size.h, COL = 1400;
type Box = { x: number; y: number; w: number; h: number };
const B = F.owner.boxes as Record<"allowance" | "runway" | "needsYou" | "why" | "table", Box>;
const right = (b: Box) => [b.x + b.w + 6, b.y + b.h / 2] as const;
const gapY = (B.allowance.y + B.allowance.h + B.runway.y) / 2;
const gapX = (B.table.x + B.table.w + B.allowance.x) / 2;
const CALLOUTS: { label: string; y: number; path: (readonly [number, number])[] }[] = [
  { label: "Allowance left today", y: right(B.allowance)[1], path: [[COL - OX - 20, right(B.allowance)[1]], right(B.allowance)] },
  { label: "Runway at this burn", y: right(B.runway)[1], path: [[COL - OX - 20, right(B.runway)[1]], right(B.runway)] },
  { label: "Needs you", y: right(B.needsYou)[1], path: [[COL - OX - 20, right(B.needsYou)[1]], right(B.needsYou)] },
  { label: "The why line under a payment", y: gapY, path: [[COL - OX - 20, gapY], [gapX, gapY], [gapX, right(B.why)[1]], right(B.why)] },
];
const Owner: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const first = Math.round((F.owner.holdTop + 0.35) * fps), each = Math.round(2.4 * fps);
  return (
    <AbsoluteFill style={{ background: C.paper }}>
      <div style={{ position: "absolute", left: OX - 2, top: OY - 2, width: FW + 4, height: FH + 4, border: `2px solid ${C.rule}`, boxSizing: "border-box" }} />
      <OffthreadVideo src={staticFile("footage/owner.mp4")} muted style={{ position: "absolute", left: OX, top: OY, width: FW, height: FH }} />
      {CALLOUTS.map((c, i) => {
        const start = first + i * each, end = i === CALLOUTS.length - 1 ? 9999 : start + each;
        if (frame < start - 1 || frame > end + 8) return null;
        const p = spring({ frame: frame - start, fps, config: { damping: 200 } });
        const out = interpolate(frame, [end, end + 8], [1, 0], clamp);
        const pts = c.path.map(([x, y]) => `${x + OX},${y + OY}`).join(" ");
        const [tx, ty] = c.path[c.path.length - 1];
        return (
          <React.Fragment key={c.label}>
            <svg width={1920} height={1080} style={{ position: "absolute", left: 0, top: 0, opacity: out }}>
              <polyline points={pts} fill="none" stroke={C.ink} strokeWidth={2} strokeDasharray={2000} strokeDashoffset={2000 * (1 - p)} />
              <circle cx={tx + OX} cy={ty + OY} r={7 * p} fill={C.ink} />
            </svg>
            <div style={{ position: "absolute", left: COL, top: c.y + OY, width: 470, transform: `translateY(-50%) translateX(${(1 - p) * 16}px)`, opacity: p * out, fontFamily: SANS, fontWeight: 500, fontSize: 40, lineHeight: 1.2, color: C.ink }}>
              {c.label}
            </div>
          </React.Fragment>
        );
      })}
    </AbsoluteFill>
  );
};

// 60-78 s: the capsule flips to Agent, the three slots; then the signature count from the proof file.
// Same place as the owner footage: the agent clip opens on the owner page, so the crossfade lands on the same frame.
const AX = OX, AY = OY;
const Agent: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const clipEnd = Math.round(F.agent.seconds * fps);
  const s = F.agent.slots;
  const z = spring({ frame: frame - Math.round((F.agent.expand + 0.6) * fps), fps, config: { damping: 200 }, durationInFrames: 36 });
  const scale = 1 + 0.6 * z;
  const tx = Math.min(0, Math.max(FW - FW * scale, FW / 2 - (s.x + s.w / 2) * scale));
  const ty = Math.min(0, Math.max(FH - FH * scale, FH / 2 - (s.y + s.h / 2) * scale));
  const videoOut = interpolate(frame, [clipEnd - 12, clipEnd], [1, 0], clamp);
  const t0 = clipEnd + 6;
  const cols = ["records", "unsigned", "single", "double", "triple"] as const;
  return (
    <AbsoluteFill style={{ background: C.sea }}>
      {frame < clipEnd + 1 && (
        <div style={{ position: "absolute", left: AX, top: AY, width: FW, height: FH, overflow: "hidden", opacity: videoOut, outline: `2px solid ${C.seaRule}` }}>
          <OffthreadVideo src={staticFile("footage/agent.mp4")} muted style={{ width: FW, height: FH, transformOrigin: "0 0", transform: `translate(${tx}px, ${ty}px) scale(${scale})` }} />
        </div>
      )}
      {frame >= t0 && (
        <div style={{ position: "absolute", left: PAD + 40, top: 150, fontFamily: MONO, color: C.seaInk2 }}>
          <Source dark at={t0}>{N.symbolon.file}:{N.symbolon.total.line}</Source>
          <div style={{ display: "grid", gridTemplateColumns: `repeat(${cols.length}, 300px)`, marginTop: 56, rowGap: 20 }}>
            {cols.map((c, i) => (
              <div key={c} style={{ fontSize: 40, opacity: fadeIn(frame, t0 + 6 + i * 4, 8), borderBottom: `2px solid ${C.seaRule}`, paddingBottom: 14 }}>{c}</div>
            ))}
            {cols.map((c, i) => (
              <Press key={c} at={t0 + 30 + i * 8}>
                <div style={{ fontSize: 96, color: C.seaInk, fontWeight: c === "double" ? 500 : 400 }}>{N.symbolon[c]}</div>
              </Press>
            ))}
          </div>
        </div>
      )}
    </AbsoluteFill>
  );
};

// 78-96 s: the four credit transactions with their hashes, then the Gateway line.
const Credit: React.FC = () => {
  const frame = useCurrentFrame();
  const gwAt = 330; // 89 s, with its caption
  return (
    <AbsoluteFill style={{ background: C.sea, padding: `110px 64px`, fontFamily: MONO, color: C.seaInk2 }}>
      <Source dark>{N.credit.file}:{N.credit.txs[0].line}-{N.credit.txs[N.credit.txs.length - 1].line}, Arc testnet {N.credit.chainId}</Source>
      <div style={{ fontSize: 36, marginTop: 14, opacity: fadeIn(frame, 8) }}>{N.credit.explorer}</div>
      {N.credit.txs.map((t, i) => {
        const at = 24 + i * 34;
        const line = `${t.step.padEnd(5)}  ${t.amount}  ${t.hash}`;
        return (
          <div key={t.step} style={{ fontSize: 36, marginTop: i ? 18 : 40, whiteSpace: "pre", color: C.seaInk }}>
            {typed(line, frame, at, 3)}
          </div>
        );
      })}
      {frame >= gwAt && (
        <div style={{ marginTop: 72 }}>
          <Source dark at={gwAt}>{N.gateway.file}:{N.gateway.line}</Source>
          <div style={{ fontSize: 36, marginTop: 14, color: C.seaInk, lineHeight: 1.32, maxWidth: 1790 }}>{typed(N.gateway.text, frame, gwAt + 6, 3)}</div>
        </div>
      )}
    </AbsoluteFill>
  );
};

// 96-110 s: the honest state, four lines.
const Honest: React.FC = () => {
  const frame = useCurrentFrame();
  const m = N.metrics;
  const lines = [
    "Testnet only.",
    "No outside operator yet.",
    "The agent has only bought from our own x402 seller.",
    `The rules decided ${m.rules.n} of ${m.passes.finished} passes, the model is bounded by the contract.`,
  ];
  let at = 12;
  return (
    <AbsoluteFill style={{ background: C.paper, padding: `150px ${PAD + 40}px`, fontFamily: SANS, color: C.ink }}>
      {lines.map((l) => {
        const start = at;
        at += Math.ceil(l.length / 1.6) + 12;
        return <div key={l} style={{ fontSize: 52, lineHeight: 1.25, marginBottom: 26, minHeight: 65 }}>{typed(l, frame, start, 1.6)}</div>;
      })}
      <div style={{ fontFamily: MONO, fontSize: 36, color: C.ink2, marginTop: 12, opacity: fadeIn(frame, at) }}>
        <div>{m.rules.text}</div>
        <div>{m.fallback.text}</div>
        <div style={{ marginTop: 8 }}>{m.file}:{m.rules.line}-{m.fallback.line}</div>
      </div>
    </AbsoluteFill>
  );
};

// 110-120 s: the end card.
const End: React.FC = () => (
  <AbsoluteFill style={{ background: C.paper, padding: `0 ${PAD + 40}px`, justifyContent: "center", color: C.ink }}>
    <Press at={4}><div style={{ fontFamily: SANS, fontWeight: 600, fontSize: 120, lineHeight: 1.1 }}>Tami</div></Press>
    <Press at={16}><div style={{ fontFamily: MONO, fontSize: 44, marginTop: 40 }}>{F.url}</div></Press>
    <Press at={24}><div style={{ fontFamily: MONO, fontSize: 44, marginTop: 16 }}>github.com/LingSiewWin/Tami</div></Press>
    <Press at={32}><div style={{ fontFamily: SANS, fontSize: 44, marginTop: 40, color: C.ink2 }}>Built for the Tameion Agents Hackathon</div></Press>
  </AbsoluteFill>
);

const Caption: React.FC<{ text: string; len: number }> = ({ text, len }) => {
  const frame = useCurrentFrame();
  const o = Math.min(fadeIn(frame, 0, 5), interpolate(frame, [len - 5, len], [1, 0], clamp));
  return (
    <AbsoluteFill style={{ justifyContent: "flex-end", alignItems: "center", paddingBottom: 44 }}>
      <div style={{ opacity: o, background: C.paper, border: `2px solid ${C.rule}`, borderRadius: 12, padding: "12px 30px", maxWidth: 1640, fontFamily: SANS, fontWeight: 500, fontSize: 44, lineHeight: 1.25, color: C.ink, textAlign: "center" }}>
        {text}
      </div>
    </AbsoluteFill>
  );
};

const VIEWS = [Title, Organiser, Tests, Owner, Agent, Credit, Honest, End];

export const Demo: React.FC = () => (
  <AbsoluteFill style={{ background: C.paper }}>
    <TransitionSeries>
      {VIEWS.map((View, i) => (
        <React.Fragment key={i}>
          {i > 0 && <TransitionSeries.Transition presentation={fade()} timing={linearTiming({ durationInFrames: FADE })} />}
          <TransitionSeries.Sequence durationInFrames={SCENES[i] + (i < VIEWS.length - 1 ? FADE : 0)} premountFor={30}>
            <View />
          </TransitionSeries.Sequence>
        </React.Fragment>
      ))}
    </TransitionSeries>
    {CUES.map((c) => (
      <Sequence key={c.id} from={c.from} durationInFrames={c.to - c.from} premountFor={30} name={`caption ${c.id}`}>
        <Caption text={c.text} len={c.to - c.from} />
        {c.voice && <Audio src={staticFile(c.voice)} />}
      </Sequence>
    ))}
  </AbsoluteFill>
);
