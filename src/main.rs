mod protocol;
mod receiver;
mod sender;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "p2p-screenshare",
    about = "P2P multi-monitor screen sharing (one stream per remote monitor)",
    long_about = "Direct peer-to-peer screen share over a single TCP connection.\n\
\n\
  share : capture every local monitor and stream them to a peer that connects to us.\n\
          Display N is sent as stream N.\n\
\n\
  view  : connect to a sharer, open one borderless-fullscreen window per remote\n\
          monitor, and place each window on a different local monitor (round-robin\n\
          if there are fewer local monitors than remote ones).\n\
\n\
Examples:\n\
  p2p-screenshare share --bind 0.0.0.0:9000\n\
  p2p-screenshare view  --connect 192.168.1.5:9000\n\
\n\
Press Esc on any view window to quit."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Capture local monitors and stream to the peer that connects to us.
    Share {
        /// host:port to bind (e.g. 0.0.0.0:9000)
        #[arg(long, default_value = "0.0.0.0:9000")]
        bind: String,
        /// Target frames-per-second per monitor.
        #[arg(long, default_value_t = 60)]
        fps: u32,
        /// JPEG quality, 1..=100.
        #[arg(long, default_value_t = 70)]
        quality: u8,
        /// Skip re-encoding when the captured frame is byte-identical to the previous one.
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        skip_unchanged: bool,
    },
    /// Connect to a sharer and display each remote monitor on its own local monitor.
    View {
        /// host:port of the sharer
        #[arg(long)]
        connect: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Share { bind, fps, quality, skip_unchanged } => {
            let q = quality.clamp(1, 100);
            let fps = fps.clamp(1, 240);
            sender::run_sender(&bind, fps, q, skip_unchanged)
        }
        Cmd::View { connect } => receiver::run_receiver(&connect),
    }
}
