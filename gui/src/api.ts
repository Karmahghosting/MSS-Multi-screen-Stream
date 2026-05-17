import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type DisplayInfo = {
  id: number;
  width: number;
  height: number;
};

export type ShareConfig = {
  port: number;
  fps: number;
  quality: number;
  skipUnchanged: boolean;
  displays: number[]; // empty → all
};

export type LogLine = { line: string; kind: "stdout" | "stderr" | "gui" };
export type StatLine = { id: number; fps: number; kbps: number };
export type ChildExit = { code: number | null };

export async function listDisplays(): Promise<DisplayInfo[]> {
  return await invoke<DisplayInfo[]>("list_displays");
}

export async function currentCode(port: number): Promise<string | null> {
  return await invoke<string | null>("current_code", { port });
}

export async function decodeCode(
  code: string
): Promise<{ host: string; port: number } | null> {
  return await invoke<{ host: string; port: number } | null>("decode_code", { code });
}

export async function startShare(cfg: ShareConfig): Promise<void> {
  await invoke("start_share", {
    cfg: {
      port: cfg.port,
      fps: cfg.fps,
      quality: cfg.quality,
      skip_unchanged: cfg.skipUnchanged,
      displays: cfg.displays,
    },
  });
}

export async function startView(connect: string): Promise<void> {
  await invoke("start_view", { connect });
}

export async function stop(): Promise<void> {
  await invoke("stop");
}

export async function isRunning(): Promise<boolean> {
  return await invoke<boolean>("is_running");
}

export function onLog(cb: (l: LogLine) => void): Promise<UnlistenFn> {
  return listen<LogLine>("log", (e) => cb(e.payload));
}

export function onStat(cb: (s: StatLine) => void): Promise<UnlistenFn> {
  return listen<StatLine>("stat", (e) => cb(e.payload));
}

export function onChildExited(cb: (x: ChildExit) => void): Promise<UnlistenFn> {
  return listen<ChildExit>("child_exited", (e) => cb(e.payload));
}
