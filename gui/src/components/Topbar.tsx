import { useEffect, useState } from "react";
import { listDisplays } from "../api";

export default function Topbar({ onHome }: { onHome: () => void }) {
  const [displays, setDisplays] = useState<number>(0);

  useEffect(() => {
    listDisplays()
      .then((d) => setDisplays(d.length))
      .catch(() => setDisplays(0));
  }, []);

  return (
    <header className="topbar">
      <div className="topbar-brand" onClick={onHome} style={{ cursor: "pointer" }}>
        <span className="logo">◆ MSS</span>
        <span className="tag">/ Multi-Screen Stream</span>
      </div>
      <div className="topbar-meta">
        <span>{displays} display{displays === 1 ? "" : "s"}</span>
        <span className="sep">·</span>
        <span>v{__APP_VERSION__}</span>
      </div>
    </header>
  );
}

declare const __APP_VERSION__: string;
