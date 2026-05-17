import { useEffect, useState } from "react";
import Home from "./screens/Home";
import ConfigureShare from "./screens/ConfigureShare";
import ConfigureView from "./screens/ConfigureView";
import Running from "./screens/Running";
import Topbar from "./components/Topbar";
import Toast from "./components/Toast";
import { isRunning, onChildExited } from "./api";

export type Page = "home" | "share-config" | "view-config" | "running";
export type Mode = "share" | "view";

export default function App() {
  const [page, setPage] = useState<Page>("home");
  const [mode, setMode] = useState<Mode>("share");
  const [toast, setToast] = useState<string | null>(null);

  // If the GUI was restarted while the CLI is still alive (shouldn't happen
  // in practice, but be defensive), surface that.
  useEffect(() => {
    isRunning().then((live) => {
      if (live) setPage("running");
    });
  }, []);

  useEffect(() => {
    let cancel: undefined | (() => void);
    onChildExited((x) => {
      const note =
        x.code === 0
          ? "Session ended."
          : `Session ended (exit code ${x.code ?? "?"}).`;
      flashToast(note, setToast);
    }).then((unlisten) => {
      cancel = unlisten;
    });
    return () => cancel?.();
  }, []);

  return (
    <div className="app">
      <Topbar onHome={() => setPage("home")} />
      <main className="content">
        {page === "home" && (
          <Home
            onShare={() => {
              setMode("share");
              setPage("share-config");
            }}
            onView={() => {
              setMode("view");
              setPage("view-config");
            }}
          />
        )}
        {page === "share-config" && (
          <ConfigureShare
            onBack={() => setPage("home")}
            onStarted={() => setPage("running")}
            flash={(m) => flashToast(m, setToast)}
          />
        )}
        {page === "view-config" && (
          <ConfigureView
            onBack={() => setPage("home")}
            onStarted={() => setPage("running")}
            flash={(m) => flashToast(m, setToast)}
          />
        )}
        {page === "running" && (
          <Running
            mode={mode}
            onHome={() => setPage("home")}
            flash={(m) => flashToast(m, setToast)}
          />
        )}
      </main>
      {toast && <Toast text={toast} />}
    </div>
  );
}

function flashToast(text: string, setToast: (t: string | null) => void) {
  setToast(text);
  setTimeout(() => setToast(null), 1800);
}
