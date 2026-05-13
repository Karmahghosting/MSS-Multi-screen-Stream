# MSS — Multi-Screen Stream

Direct P2P multi-monitor screen sharing in Rust. Each remote monitor lands on
one of your local monitors as a borderless-fullscreen window: share three
screens, the viewer sees three screens, one per physical display.

## Highlights

- **One stream per remote monitor.** A sharer with N displays emits N tracks
  multiplexed over a single TCP connection; the viewer opens one borderless
  fullscreen window per stream and pins each to a different local monitor
  (round-robin when local < remote).
- **60 fps target.** JPEG encode via [`jpeg-encoder`](https://crates.io/crates/jpeg-encoder)
  ingesting BGRA directly (no per-frame color conversion). Decode via
  [`zune-jpeg`](https://crates.io/crates/zune-jpeg) emitting BGRA, blitted to
  the window with a 1:1 `copy_from_slice` fast path when sizes match.
- **Idle ≈ 0 CPU.** An xxh3 hash over each captured frame skips re-encoding
  whenever the screen is byte-identical to the previous one.
- **Latest-frame-wins.** Capture and decode never queue stale frames; a slow
  network silently drops old frames in favor of fresh ones.
- **Parallel decode.** One decode worker per remote monitor.
- **Modern launcher GUI.** Wizard flow (Home → Configure → Running), live
  per-monitor fps/KB/s sparklines, recent connections, persisted settings.
- **Single multiplexed TCP connection.** No relay, no STUN, no signaling
  server. Direct peer-to-peer on LAN; tunnel (Tailscale / WireGuard / SSH /
  ngrok TCP) for the open Internet.

## Install

### From a release

Download the latest archive for your platform from the
[Releases](https://github.com/Karmahghosting/MSS-Multi-screen-Stream/releases)
page. Each archive contains both binaries:

- `p2p-screenshare` — the CLI
- `p2p-screenshare-gui` — the launcher GUI

### From source

```bash
git clone https://github.com/Karmahghosting/MSS-Multi-screen-Stream.git
cd MSS-Multi-screen-Stream
cargo build --release
```

Binaries land in `target/release/`.

#### Linux build dependencies

```bash
sudo apt-get install \
  libxcb1-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libxkbcommon-x11-dev \
  libgtk-3-dev libssl-dev libwayland-dev \
  libfontconfig1-dev pkg-config
```

## Usage

### GUI

Run `p2p-screenshare-gui`. Pick **Share** or **View** on the home screen,
configure (fps / quality / bind or connect address), hit **Start**. The GUI
spawns the CLI as a child and shows live stats parsed from its output.

### CLI

On the sharing machine:

```bash
p2p-screenshare share --bind 0.0.0.0:9000 --fps 60 --quality 70
```

On the viewing machine:

```bash
p2p-screenshare view --connect <sharer-ip>:9000
```

Press `Esc` in any viewer window to quit.

### CLI flags

```
share
  --bind <host:port>          default 0.0.0.0:9000
  --fps <1..=120>             default 60
  --quality <1..=100>         JPEG quality, default 70
  --skip-unchanged <bool>     xxh3 frame-identity skip, default true

view
  --connect <host:port>       required
```

## Wire format

Direct TCP, one connection per session. The sharer is the listener, the viewer
is the connector.

```
Handshake (sharer → viewer, once):
  magic     u32 LE = 0x5350_3250   // "P2PS"
  version   u8     = 1
  monitors  u8                     // how many tracks follow

Frame (sharer → viewer, repeating):
  monitor_id  u8
  width       u32 LE
  height      u32 LE
  data_len    u32 LE
  data        [u8; data_len]       // full intra-frame JPEG
```

There is no audio, no signaling, no key-frame negotiation: every frame is an
independent JPEG.

## Architecture

```
sender                                                            receiver
──────                                                            ────────
 ┌─────────────┐                                                  ┌────────────────┐
 │ capture[0]  │──┐    BGRA pack → xxh3 → JPEG (jpeg-encoder)     │  decode[0]     │── softbuffer present
 ├─────────────┤  │                                               ├────────────────┤   on monitor 0 (1:1 fast path)
 │ capture[1]  │──┼─► latest-wins slots ─► single TCP writer ─►   │  decode[1]     │── monitor 1
 ├─────────────┤  │                       ▲                       ├────────────────┤
 │ capture[N]  │──┘                       │                       │  decode[N]     │── monitor N
 └─────────────┘                          │                       └────────────────┘
                                          │
                                  one TCP connection
```

- Per-monitor capture + JPEG encode threads on the sharer; a single I/O thread
  drains "latest-wins" slots and writes to the socket.
- A single network thread on the viewer demultiplexes by `monitor_id` and pushes
  the encoded payload into per-monitor decode workers.
- Each decode worker writes decoded BGRA into an `Arc<Vec<u32>>` slot. The UI
  thread blits the slot to the corresponding fullscreen window with a 1:1
  memcpy when sizes match, or a nearest-neighbor pass through a precomputed
  scale lookup table otherwise.

## Platform support

| OS                | Capture                          | GUI | Notes                                                      |
| ----------------- | -------------------------------- | --- | ---------------------------------------------------------- |
| Windows 10 / 11   | DXGI Desktop Duplication         | ✓   | recommended                                                |
| macOS 10.13+      | CoreGraphics                     | ✓   | grant **Screen Recording** permission in System Settings   |
| Linux (X11)       | XLib                             | ✓   | install build deps above                                   |
| Linux (Wayland)   | not yet                          | ✓   | the GUI runs; capture not supported by the current backend |

Capture today goes through [`scrap`](https://crates.io/crates/scrap); the
Wayland gap is the main known limitation.

## Performance notes

- 1080p × 3 monitors at 60 fps fits comfortably on any recent x86 CPU
  (≈3–8 ms per JPEG at quality 70).
- 1440p × 3 at 60 fps is tight but workable.
- 4K × 3 at 60 fps is encoder-bound (≈12–20 ms per JPEG); in practice you get
  30–45 effective fps. Hardware H.264/HEVC (NVENC, QuickSync, VideoToolbox)
  would lift the ceiling — the wire format would need a tiny extension to
  carry NALUs instead of JPEGs.

## CI / Releases

- [`ci.yml`](.github/workflows/ci.yml) — `cargo check`, `cargo fmt`, `cargo
  clippy` on Linux / macOS / Windows for every push and PR.
- [`release.yml`](.github/workflows/release.yml) — pushing a `v*` tag builds
  binaries for `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`,
  `x86_64-apple-darwin`, `aarch64-apple-darwin` and attaches them to a GitHub
  Release with auto-generated notes.

Cut a release with:

```bash
git tag v0.2.0
git push origin v0.2.0
```

## License

MIT. See [LICENSE](LICENSE) (or treat the absence of a `LICENSE` file as a
TODO — the code is offered under MIT terms regardless).
