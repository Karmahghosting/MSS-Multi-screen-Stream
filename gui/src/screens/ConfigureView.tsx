import { useEffect, useState } from "react";
import { decodeCode, startView } from "../api";

type Props = {
  onBack: () => void;
  onStarted: () => void;
  flash: (msg: string) => void;
};

const RECENT_KEY = "mss.recent_codes";
const LAST_CODE = "mss.connect_code";
const LAST_HOSTPORT = "mss.connect_hostport";

function formatCode(raw: string): string {
  const clean = raw
    .toUpperCase()
    .replace(/[^A-Z2-9]/g, "")
    .slice(0, 10);
  return clean.length > 5 ? `${clean.slice(0, 5)}-${clean.slice(5)}` : clean;
}

export default function ConfigureView({ onBack, onStarted, flash }: Props) {
  const [code, setCode] = useState<string>(() => {
    try {
      return formatCode(localStorage.getItem(LAST_CODE) || "");
    } catch {
      return "";
    }
  });
  const [recents, setRecents] = useState<string[]>(() => {
    try {
      return JSON.parse(localStorage.getItem(RECENT_KEY) || "[]");
    } catch {
      return [];
    }
  });
  const [hostPort, setHostPort] = useState<string>(() => {
    try {
      return localStorage.getItem(LAST_HOSTPORT) || "";
    } catch {
      return "";
    }
  });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    try {
      localStorage.setItem(LAST_CODE, code);
    } catch {
      /* ignore */
    }
  }, [code]);

  const pushRecent = (c: string) => {
    const next = [c, ...recents.filter((r) => r !== c)].slice(0, 8);
    setRecents(next);
    try {
      localStorage.setItem(RECENT_KEY, JSON.stringify(next));
    } catch {
      /* ignore */
    }
  };

  const connectByCode = async () => {
    if (busy) return;
    const decoded = await decodeCode(code).catch(() => null);
    if (!decoded) {
      setError(
        "That code doesn't look right. Codes are 10 letters/digits like XXXXX-XXXXX (no 0, 1, I or O)."
      );
      return;
    }
    setError(null);
    pushRecent(formatCode(code));
    setBusy(true);
    try {
      await startView(`${decoded.host}:${decoded.port}`);
      onStarted();
    } catch (e) {
      flash(`Couldn't connect: ${e}`);
      setBusy(false);
    }
  };

  const connectByHostPort = async () => {
    if (busy || !hostPort.trim()) return;
    try {
      localStorage.setItem(LAST_HOSTPORT, hostPort);
    } catch {
      /* ignore */
    }
    setBusy(true);
    try {
      await startView(hostPort.trim());
      onStarted();
    } catch (e) {
      flash(`Couldn't connect: ${e}`);
      setBusy(false);
    }
  };

  return (
    <>
      <div className="row" style={{ marginBottom: 16 }}>
        <button className="btn-ghost" onClick={onBack}>
          ←  Home
        </button>
        <h2 style={{ margin: 0, fontSize: 20 }}>View — configure</h2>
      </div>

      <p className="section-label">Session code</p>
      <div className="card hero stack">
        <p className="muted" style={{ margin: 0 }}>
          Ask the person sharing for their code, then type it here.
        </p>
        <div className="code-input">
          <input
            value={code}
            onChange={(e) => {
              setCode(formatCode(e.target.value));
              setError(null);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") connectByCode();
            }}
            placeholder="XXXXX-XXXXX"
            spellCheck={false}
            autoFocus
          />
        </div>
        {error && <div className="error-text">{error}</div>}
      </div>

      {recents.length > 0 && (
        <>
          <div style={{ height: 18 }} />
          <p className="section-label">Recent</p>
          <div className="card stack">
            {recents.map((c) => (
              <a
                key={c}
                onClick={() => {
                  setCode(formatCode(c));
                  setError(null);
                }}
              >
                <span style={{ marginRight: 8 }}>•</span>
                <code style={{ fontSize: 15 }}>{c}</code>
              </a>
            ))}
          </div>
        </>
      )}

      <div style={{ height: 18 }} />

      <details className="advanced">
        <summary>Advanced — connect by host:port</summary>
        <div className="advanced-body">
          <label className="field">
            <span>Connect to</span>
            <input
              type="text"
              placeholder="192.168.1.5:9000"
              value={hostPort}
              onChange={(e) => setHostPort(e.target.value)}
            />
          </label>
          <button onClick={connectByHostPort} disabled={busy || !hostPort.trim()}>
            Connect by host:port  →
          </button>
        </div>
      </details>

      <div style={{ height: 18 }} />

      <button
        className="btn-primary btn-big"
        onClick={connectByCode}
        disabled={busy || code.replace("-", "").length < 10}
      >
        Connect  →
      </button>
    </>
  );
}
