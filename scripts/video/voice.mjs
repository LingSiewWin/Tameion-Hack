#!/usr/bin/env node
// The spoken lines of the demo video: one place for their words and start times. Writes
//   apps/video/public/voice/NN.mp3   macOS `say -v Samantha -r 175`, converted with ffmpeg (skipped with --no-voice)
//   apps/video/src/cues.json         start/end frame of every caption (what Remotion burns in) and its mp3, if any
//   apps/video/out/tami-demo.srt     the same cues for YouTube
// A caption stays up for its voice plus 0.6 s, at least (words x 0.3 + 1) s, and never past the next cue or its scene.
// Usage: node scripts/video/voice.mjs [--no-voice]
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync, rmSync, existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const APP = join(ROOT, "apps", "video");
const FFMPEG = "/opt/homebrew/bin/ffmpeg", FFPROBE = "/opt/homebrew/bin/ffprobe";
const FPS = 30;
const VOICE = !process.argv.includes("--no-voice");

// at: seconds from the start; until: the scene's end (a caption never crosses a scene cut); say: words for the voice
// where the caption's spelling would be read wrong (each one checked by transcribing the render with whisper-cli).
const LINES = [
  { at: 2.4, until: 7, text: "Built for the Tameion Agents Hackathon." },
  { at: 14.0, until: 18, text: "Here are those three, as tests." },
  { at: 20.8, until: 38, text: "Wrong vendor: refused by the contract, nothing sent." },
  { at: 26.0, until: 38, text: "Phantom invoice: 422, not in the record.", say: "Phantom invoice. Four twenty-two. Not in the record." },
  { at: 31.2, until: 38, text: "Double pay on retry: one key sent four times, one payment." },
  { at: 38.8, until: 60, text: "The live demo account, recorded today." },
  { at: 49.8, until: 60, text: "The owner sees dollars, days and what needs them. Not one hash." },
  { at: 61.0, until: 78, text: "Every record has three signature slots." },
  { at: 64.8, until: 78, text: "The chain's half is read back." },
  { at: 68.3, until: 78, text: "The seller's half arrives with a signed invoice." },
  { at: 72.6, until: 78, text: "The agent's half is next." },
  { at: 79.0, until: 96, text: "Credit drawn against the same book on Arc testnet." },
  { at: 83.8, until: 96, text: "No external lender. Not a lending protocol." },
  { at: 89.0, until: 96, text: "The Gateway balance is the second witness.", say: "The Gateway balance [[slnc 150]] is the second witness." },
  { at: 104.0, until: 110, text: "Every line on screen has a file in proof/.", say: "Every line on screen has a file in proof slash." },
];

const dir = join(APP, "public", "voice");
if (existsSync(dir)) rmSync(dir, { recursive: true });
if (VOICE) mkdirSync(dir, { recursive: true });
const tmp = join(APP, "out", "voice-aiff");
mkdirSync(tmp, { recursive: true });

const seconds = (f) => Number(execFileSync(FFPROBE, ["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", f]).toString());
const cues = LINES.map((l, i) => {
  const id = String(i + 1).padStart(2, "0");
  let voice = 0, src = null;
  if (VOICE) {
    const aiff = join(tmp, `${id}.aiff`), mp3 = join(dir, `${id}.mp3`);
    execFileSync("say", ["-v", "Samantha", "-r", "175", "-o", aiff, l.say ?? l.text]);
    execFileSync(FFMPEG, ["-y", "-loglevel", "error", "-i", aiff, "-ac", "1", "-ar", "44100", "-b:a", "128k", mp3]);
    voice = seconds(mp3);
    src = `voice/${id}.mp3`;
  }
  const next = LINES[i + 1]?.at ?? Infinity;
  const limit = Math.min(next - 0.15, l.until - 0.2);
  if (l.at + voice > limit) throw new Error(`line ${id} "${l.text}": the voice runs ${voice.toFixed(2)} s, only ${(limit - l.at).toFixed(2)} s before the next cue`);
  const read = l.text.split(/\s+/).length * 0.3 + 1;
  const end = Math.min(limit, l.at + Math.max(voice + 0.6, read));
  return { id, text: l.text, from: Math.round(l.at * FPS), to: Math.round(end * FPS), voice: src, voiceSeconds: +voice.toFixed(3) };
});
rmSync(tmp, { recursive: true });

writeFileSync(join(APP, "src", "cues.json"), JSON.stringify(cues, null, 2) + "\n");
const ts = (f) => {
  const ms = Math.round((f / FPS) * 1000);
  const p = (n, w = 2) => String(n).padStart(w, "0");
  return `${p(Math.floor(ms / 3600000))}:${p(Math.floor(ms / 60000) % 60)}:${p(Math.floor(ms / 1000) % 60)},${p(ms % 1000, 3)}`;
};
writeFileSync(join(APP, "out", "tami-demo.srt"), cues.map((c, i) => `${i + 1}\n${ts(c.from)} --> ${ts(c.to)}\n${c.text}\n`).join("\n"));
for (const c of cues) console.log(`${c.id} ${(c.from / FPS).toFixed(1)}-${(c.to / FPS).toFixed(1)} s  voice ${c.voiceSeconds.toFixed(2)} s  ${c.text}`);
