import { Composition } from "remotion";
import { Demo } from "./Demo";

export const RemotionRoot: React.FC = () => (
  <Composition id="Demo" component={Demo} durationInFrames={3600} fps={30} width={1920} height={1080} />
);
