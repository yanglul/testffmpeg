use std::path::PathBuf;

use futures::{future::OptionFuture, FutureExt};

use crate::play::audio;
use crate::play::video;

#[derive(Clone, Copy)]
pub enum ControlCommand {
    Play,
    Pause,
}

pub struct Player {
    control_sender: smol::channel::Sender<ControlCommand>,
    demuxer_thread: Option<std::thread::JoinHandle<()>>,
    playing: bool,
    playing_changed_callback: Box<dyn Fn(bool)>,
}

