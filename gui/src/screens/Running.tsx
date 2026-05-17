import { useEffect, useRef, useState } from "react";
import { onLog, onStat, stop, type LogLine } from "../api";
import type { Mode } from "../App";

type Props = {
  mode: Mode;
  onHome: () => void;
  flash: (msg: string) => void;
};

type MonitorStat = {
  fps: number;
  kbps: number;
  history: { fps: number; t: number }[];
  lastUpdate: number;
};

const HISTORY = 60;

export default function Running({ mode, onHome, flash: _flash }: Props) {
  const [live, setLive] = useState(true);
  const [log, setLog] = useState<LogLine[]>([]);
  const [stats, setStats] = useState<Record<number, MonitorStat>>({});
  // We don't read the tick value; we just need a re-render every second so
  // the "elapsed" display updates.
  const [, bumpTick] = useState(0);
  const startedAt = useRef(Date.now());

  useEffect(() => {
    let unlistenLog: undefined | (() => void);
    let unlistenStat: undefined | (() => void);
    onLog((l) => {
      setLog((cur) => {
        const next = [...cur, l];
        if (next.length > 600) next.splice(0, next.length - 600);
        return next;
      });
    }).then((u) => (unlistenLog = u));
    onStat((s) => {
      setStats((cur) => {
        const prev = cur[s.id] ?? {
          fps: 0,
          kbps: 0,
          history: [],
          lastUpdate: 0,
        };
        const history = [...prev.history, { fps: s.fps, t: Date.now() }];
        while (history.length > HISTORY) history.shift();
        return {
          ...cur,
          [s.id]: {
            fps: s.fps,
            kbps: s.kbps,
            history,
            lastUpdate: Date.now(),
          },
        };
      });
    }).then((u) => (unlistenStat = u));
    return () => {
      unlistenLog?.();
      unlistenStat?.();
    };
  }, []);

  useEffect(() => {
    const t = setInterval(() => bumpTick((x) => x + 1), 1000);
    return () => clearInterval(t);
  }, []);

  const onStop = async () => {
    try {
      await stop();
    } catch {
      /* ignore */
    }
    setLive(false);
  };

  const elapsed = Math.floor((Date.now() - startedAt.current) / 1000);
  const mm = String(Math.floor(elapsed / 60)).padStart(2, "0");
  const ss = String(elapsed % 60).padStart(2, "0");

  const ids = Object.keys(stats)
    .map(Number)
    .sort((a, b) => a - b);

  return (
    <>
      <div className="status-bar">
        <div className={"status-pill" + (live ? "" : " off")}>
          <span className="status-dot" />
          {live ? "LIVE" : "STOPPED"}
        </div>
        <div className="muted small">
          {mode === "share" ? "Share session" : "View session"}
        </div>
        <div className="muted small" style={{ fontFamily: "ui-monospace, monospace" }}>
          ⏱ {mm}:{ss}
        </div>
        <span className="spacer" />
        {live ? (
          <button className="btn-danger" onClick={onStop}>
            ◼  Stop
          </button>
        ) : (
          <button onClick={onHome}>←  Home</button>
        )}
      </div>

      <div style={{ height: 16 }} />

      <p className="section-label">Per-monitor performance</p>
      <div className="card">
        {ids.length === 0 ? (
          <div className="muted">⌛  Waiting for first frame…</div>
        ) : (
          ids.map((id) => {
            const m = stats[id];
            const fresh = Date.now() - m.lastUpdate < 3000;
            return (
              <div key={id} className="monitor-row">
                <div className="id" style={{ opacity: fresh ? 1 : 0.45 }}>
                  m{id}
                </div>
                <div className="fps">{m.fps.toFixed(1)} fps</div>
                <div className="kbps">{m.kbps.toFixed(1)} KB/s</div>
                <Sparkline values={m.history.map((h) => h.fps)} />
              </div>
            );
          })
        )}
      </div>

      <div style={{ height: 16 }} />

      <p className="section-label">Log</p>
      <div className="log">
        {log.length === 0 ? (
          <div className="line subtle">(no output yet)</div>
        ) : (
          log.map((l, i) => (
            <div key={i} className={"line " + classifyLine(l)}>
              {l.kind === "stderr" ? "[err] " : ""}
              {l.line}
            </div>
          ))
        )}
      </div>
    </>
  );
}

function classifyLine(l: LogLine): string {
  if (l.kind === "stderr") return "err";
  if (l.kind === "gui") return "gui";
  if (l.line.startsWith("[share]")) return "share";
  if (l.line.startsWith("[view")) return "view";
  return "";
}

function Sparkline({ values }: { values: number[] }) {
  if (values.length === 0) {
    return <svg className="spark" />;
  }
  const w = 240;
  const h = 28;
  const max = Math.max(1, ...values);
  const dx = w / Math.max(1, values.length - 1);
  const d = values
    .map((v, i) => {
      const x = i * dx;
      const y = h - (v / max) * (h - 2) - 1;
      return `${i === 0 ? "M" : "L"} ${x.toFixed(1)} ${y.toFixed(1)}`;
    })
    .join(" ");
  return (
    <svg className="spark" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none">
      <path d={d} fill="none" stroke="var(--accent)" strokeWidth="1.5" />
    </svg>
  );
}
