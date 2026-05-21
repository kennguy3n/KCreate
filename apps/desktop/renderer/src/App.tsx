import { useCallback, useMemo, useState } from "react";

import { CanvasHost } from "./components/CanvasHost";
import type { Scene } from "../../shared/scene";

export function App(): JSX.Element {
  const [size] = useState({ width: 1024, height: 640 });
  const [fps, setFps] = useState<number>(0);
  const [lastTickAt, setLastTickAt] = useState<number>(performance.now());

  const scene: Scene = useMemo<Scene>(
    () => ({
      clear_color: [0.12, 0.12, 0.14, 1.0],
      objects: [
        {
          id: 1,
          z: 0,
          translation: [80, 80],
          style: {
            fill: [0.92, 0.36, 0.36, 1.0],
            stroke: { color: [0.0, 0.0, 0.0, 1.0], width: 2.0 },
          },
          kind: { type: "rect", x: 0, y: 0, width: 360, height: 220 },
        },
        {
          id: 2,
          z: 1,
          translation: [560, 220],
          style: {
            fill: [0.35, 0.78, 0.95, 1.0],
            stroke: null,
          },
          kind: { type: "circle", cx: 0, cy: 0, radius: 140 },
        },
      ],
    }),
    [],
  );

  const onFrame = useCallback(() => {
    const now = performance.now();
    const elapsed = now - lastTickAt;
    setLastTickAt(now);
    if (elapsed > 0) setFps(Math.round(1000 / elapsed));
  }, [lastTickAt]);

  return (
    <div className="kcreate-shell">
      <header className="kcreate-titlebar">
        <span>KCreate · phase 0 renderer</span>
        <span>{fps} fps</span>
      </header>
      <main>
        <CanvasHost
          width={size.width}
          height={size.height}
          scene={scene}
          onFramePresented={onFrame}
        />
      </main>
    </div>
  );
}
