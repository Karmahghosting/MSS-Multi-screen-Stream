import { useEffect, useState } from "react";

type Props = {
  onShare: () => void;
  onView: () => void;
};

const RECENT_CODES_KEY = "mss.recent_codes";

export default function Home({ onShare, onView }: Props) {
  const [recents, setRecents] = useState<string[]>([]);

  useEffect(() => {
    try {
      const raw = localStorage.getItem(RECENT_CODES_KEY);
      if (raw) setRecents(JSON.parse(raw));
    } catch {
      /* ignore */
    }
  }, []);

  return (
    <>
      <section className="home-hero">
        <h1>Stream screens, peer-to-peer.</h1>
        <p>
          Every remote monitor lands on one of your local monitors, fullscreen.
        </p>
      </section>

      <section className="home-cards">
        <div className="action-card" onClick={onShare}>
          <div className="row">
            <div className="icon">📡</div>
            <div>
              <h2 className="title">Share</h2>
              <div className="subtitle">Sender</div>
            </div>
          </div>
          <p className="desc">
            Capture your screens and stream them to a connecting peer.
            Pick exactly which monitors you want to send.
          </p>
          <button className="btn-primary btn-big">Configure  →</button>
        </div>

        <div className="action-card" onClick={onView}>
          <div className="row">
            <div className="icon">👁</div>
            <div>
              <h2 className="title">View</h2>
              <div className="subtitle">Receiver</div>
            </div>
          </div>
          <p className="desc">
            Type a session code and every remote monitor opens fullscreen on one
            of your local monitors. Esc to quit.
          </p>
          <button className="btn-big">Configure  →</button>
        </div>
      </section>

      {recents.length > 0 && (
        <section style={{ marginTop: 28 }}>
          <p className="section-label">Recent codes</p>
          <div className="card stack">
            {recents.map((c) => (
              <a
                key={c}
                onClick={() => {
                  localStorage.setItem("mss.connect_code", c);
                  onView();
                }}
              >
                <span style={{ marginRight: 8 }}>•</span>
                <code style={{ fontSize: 15 }}>{c}</code>
              </a>
            ))}
          </div>
        </section>
      )}
    </>
  );
}
