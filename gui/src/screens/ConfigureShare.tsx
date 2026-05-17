import { useEffect, useState } from "react";
import { currentCode, listDisplays, startShare, type DisplayInfo } from "../api";

type Props = {
  onBack: () => void;
  onStarted: () => void;
  flash: (msg: string) => void;
};

const SETTINGS_KEY = "mss.share_settings";

type ShareSettings = {
  port: number;
  fps: number;
  quality: number;
  skipUnchanged: boolean;
  selected: number[]; // empty → all
};

const DEFAULTS: ShareSettings = {
  port: 9000,
  fps: 60,
  quality: 70,
  skipUnchanged: true,
  selected: [],
};

function loadSettings(): ShareSettings {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    if (raw) return { ...DEFAULTS, ...JSON.parse(raw) };
  } catch {
    /* ignore */
  }
  return DEFAULTS;
}

function saveSettings(s: ShareSettings) {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(s));
  } catch {
    /* ignore */
  }
}

export default function ConfigureShare({ onBack, onStarted, flash }: Props) {
  const [settings, setSettings] = useState<ShareSettings>(loadSettings);
  const [displays, setDisplays] = useState<DisplayInfo[]>([]);
  const [code, setCode] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    saveSettings(settings);
  }, [settings]);

  useEffect(() => {
    listDisplays()
      .then((d) => {
        setDisplays(d);
        // Ensure no stale selected indices.
        setSettings((s) => ({
          ...s,
          selected: s.selected.filter((i) => i < d.length),
        }));
      })
      .catch(() => setDisplays([]));
  }, []);

  useEffect(() => {
    currentCode(settings.port)
      .then((c) => setCode(c))
      .catch(() => setCode(null));
    const t = setInterval(
      () => currentCode(settings.port).then(setCode).catch(() => {}),
      4000
    );
    return () => clearInterval(t);
  }, [settings.port]);

  const allSelected =
    displays.length > 0 &&
    (settings.selected.length === 0 ||
      settings.selected.length === displays.length);

  const isSelected = (i: number) =>
    settings.selected.length === 0 || settings.selected.includes(i);

  const toggle = (i: number) => {
    setSettings((s) => {
      // If selection was implicit-all, convert to explicit first so the user
      // can deselect a single monitor without unselecting everything.
      const base =
        s.selected.length === 0
          ? displays.map((_, idx) => idx)
          : [...s.selected];
      const next = base.includes(i)
        ? base.filter((x) => x !== i)
        : [...base, i].sort((a, b) => a - b);
      return { ...s, selected: next };
    });
  };

  const selectAll = () =>
    setSettings((s) => ({ ...s, selected: [] }));

  const start = async () => {
    if (busy) return;
    if (displays.length > 0 && settings.selected.length === 0 && !allSelected) {
      // Defensive — should never hit since allSelected is true when selected=[]
      flash("Pick at least one display.");
      return;
    }
    setBusy(true);
    try {
      await startShare({
        port: settings.port,
        fps: settings.fps,
        quality: settings.quality,
        skipUnchanged: settings.skipUnchanged,
        displays: settings.selected,
      });
      onStarted();
    } catch (e) {
      flash(`Couldn't start share: ${e}`);
      setBusy(false);
    }
  };

  const copyCode = async () => {
    if (!code) return;
    try {
      await navigator.clipboard.writeText(code);
      flash("Code copied.");
    } catch {
      flash("Couldn't copy — select and copy manually.");
    }
  };

  return (
    <>
      <div className="row" style={{ marginBottom: 16 }}>
        <button className="btn-ghost" onClick={onBack}>
          ←  Home
        </button>
        <h2 style={{ margin: 0, fontSize: 20 }}>Share — configure</h2>
      </div>

      <p className="section-label">Session code</p>
      <div className="card hero stack">
        <p className="muted" style={{ margin: 0 }}>
          Your peer types this on their machine — and that's it.
        </p>
        <div className="session-code">
          {code ? (
            <>
              <span className="code">{code}</span>
              <button className="btn-primary btn-icon" onClick={copyCode}>
                ⧉ Copy
              </button>
            </>
          ) : (
            <span className="muted">
              ⚠  No LAN interface detected. Connect to a network or use a host:port directly.
            </span>
          )}
        </div>
      </div>

      <div style={{ height: 18 }} />

      <p className="section-label">Choose displays</p>
      <div className="card stack">
        <div className="row">
          <span className="muted small">
            {displays.length} detected · click to toggle
          </span>
          <span className="spacer" />
          <button
            className="btn-ghost small"
            onClick={selectAll}
            disabled={allSelected}
          >
            Select all
          </button>
        </div>
        {displays.length === 0 ? (
          <div className="muted">No displays detected yet.</div>
        ) : (
          <div className="display-grid">
            {displays.map((d) => (
              <DisplayCell
                key={d.id}
                d={d}
                selected={isSelected(d.id)}
                onClick={() => toggle(d.id)}
              />
            ))}
          </div>
        )}
      </div>

      <div style={{ height: 18 }} />

      <details className="advanced">
        <summary>Advanced</summary>
        <div className="advanced-body">
          <label className="field">
            <span>Port</span>
            <input
              type="number"
              min={1}
              max={65535}
              value={settings.port}
              onChange={(e) =>
                setSettings((s) => ({
                  ...s,
                  port: Math.max(1, Math.min(65535, +e.target.value || 0)),
                }))
              }
            />
          </label>
          <label className="field">
            <span>Target FPS</span>
            <input
              type="range"
              min={1}
              max={120}
              value={settings.fps}
              onChange={(e) =>
                setSettings((s) => ({ ...s, fps: +e.target.value }))
              }
            />
          </label>
          <div className="row" style={{ marginTop: -8, marginLeft: 142 }}>
            <span className="small muted">{settings.fps} fps</span>
          </div>
          <label className="field">
            <span>JPEG quality</span>
            <input
              type="range"
              min={1}
              max={100}
              value={settings.quality}
              onChange={(e) =>
                setSettings((s) => ({ ...s, quality: +e.target.value }))
              }
            />
          </label>
          <div className="row" style={{ marginTop: -8, marginLeft: 142 }}>
            <span className="small muted">q = {settings.quality}</span>
          </div>
          <label className="field">
            <span>Skip unchanged</span>
            <label className="row" style={{ gap: 8 }}>
              <input
                type="checkbox"
                checked={settings.skipUnchanged}
                onChange={(e) =>
                  setSettings((s) => ({
                    ...s,
                    skipUnchanged: e.target.checked,
                  }))
                }
              />
              <span className="small muted">xxh3 frame hash · idle ≈ 0 CPU</span>
            </label>
          </label>
        </div>
      </details>

      <div style={{ height: 18 }} />

      <button className="btn-primary btn-big" onClick={start} disabled={busy}>
        Start sharing  →
      </button>
    </>
  );
}

function DisplayCell({
  d,
  selected,
  onClick,
}: {
  d: DisplayInfo;
  selected: boolean;
  onClick: () => void;
}) {
  // Build a roughly proportional preview of the display, fit into a 16:10 box.
  const aspect = d.width / d.height;
  const previewW = aspect >= 16 / 10 ? 84 : 84 * (aspect / (16 / 10));
  const previewH = aspect >= 16 / 10 ? 84 / aspect : 84 * (10 / 16);
  return (
    <div
      className={"display-cell" + (selected ? " selected" : "")}
      onClick={onClick}
    >
      <div className="preview">
        {selected && <span className="badge">ON</span>}
        <div
          className="preview-frame"
          style={{ width: previewW, height: previewH }}
        />
      </div>
      <div className="display-title">Display {d.id}</div>
      <div className="display-meta">
        {d.width} × {d.height}
      </div>
    </div>
  );
}
