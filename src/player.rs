use atomic::Atomic;
use ffmpeg_next as ffmpeg;
use ffmpeg::format::context::input::Input;
use egui::{ColorImage,Color32};
use ::egui::{TextureHandle,Vec2,TextureOptions};
use timer::{Guard, Timer};
use bytemuck::NoUninit;
use std::sync::{Arc,Mutex};

#[derive(Clone, Debug)]
/// Simple concurrecy of primitive values.
pub struct Shared<T: Copy + bytemuck::NoUninit> {
    raw_value: Arc<Atomic<T>>,
}

impl<T: Copy + bytemuck::NoUninit> Shared<T> {
    /// Set the value.
    pub fn set(&self, value: T) {
        self.raw_value.store(value, atomic::Ordering::Relaxed)
    }
    /// Get the value.
    pub fn get(&self) -> T {
        self.raw_value.load(atomic::Ordering::Relaxed)
    }
    /// Make a new cache.
    pub fn new(value: T) -> Self {
        Self {
            raw_value: Arc::new(Atomic::new(value)),
        }
    }
}

 

pub struct PlayerOptions {
    /// Should the stream loop if it finishes?
    pub looping: bool,
    /// The volume of the audio stream.
    pub audio_volume: Shared<f32>,
    /// The maximum volume of the audio stream.
    pub max_audio_volume: f32,
    /// The texture options for the displayed video frame.
    pub texture_options: TextureOptions,
}


impl Default for PlayerOptions {
    fn default() -> Self {
        Self {
            looping: true,
            max_audio_volume: 1.,
            audio_volume: Shared::new(0.5),
            texture_options: TextureOptions::default(),
        }
    }
}

impl PlayerOptions {
    /// Set the maxmimum player volume, and scale the actual player volume to the
    /// same current ratio.
    pub fn set_max_audio_volume(&mut self, volume: f32) {
        self.audio_volume
            .set(self.audio_volume.get() * (volume / self.max_audio_volume));
        self.max_audio_volume = volume;
    }

    /// Set the player volume, clamped in `0.0..=max_audio_volume`.
    pub fn set_audio_volume(&mut self, volume: f32) {
        self.audio_volume
            .set(volume.clamp(0., self.max_audio_volume));
    }
}





struct StreamInfo {
    // Not the actual `StreamIndex` of the stream. This is a user-facing number that starts
    // at `1` and is incrememted when cycling between streams.
    current_stream: usize,
    total_streams: usize,
}

impl StreamInfo {
    fn new() -> Self {
        Self {
            current_stream: 1,
            total_streams: 0,
        }
    }
    fn from_total(total: usize) -> Self {
        let mut slf = Self::new();
        slf.total_streams = total;
        slf
    }
    fn cycle(&mut self) {
        self.current_stream = ((self.current_stream + 1) % (self.total_streams + 1)).max(1);
    }
    fn is_cyclable(&self) -> bool {
        self.total_streams > 1
    }
}

impl std::fmt::Display for StreamInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.current_stream, self.total_streams)
    }
}
use ffmpeg_next::media::Type;
enum PlayerMessage {
    StreamCycled(Type),
}

type PlayerMessageSender = std::sync::mpsc::Sender<PlayerMessage>;
type PlayerMessageReciever = std::sync::mpsc::Receiver<PlayerMessage>;



pub struct FFMpegPlayer {
    /// The video streamer of the player.
    pub video_streamer: Arc<Mutex<VideoStreamer>>,
    /// The audio streamer of the player. Won't exist unless [`Player::with_audio`] is called and there exists
    /// a valid audio stream in the file.
    // pub audio_streamer: Option<Arc<Mutex<AudioStreamer>>>,
    /// The subtitle streamer of the player. Won't exist unless [`Player::with_subtitles`] is called and there exists
    /// a valid subtitle stream in the file.
    // pub subtitle_streamer: Option<Arc<Mutex<SubtitleStreamer>>>,
    /// The state of the player.
    pub player_state: Shared<PlayerState>,
    /// The player's texture handle.
    pub texture_handle: TextureHandle,
    /// The size of the video stream.
    pub size: Vec2,
    /// The total duration of the stream, in milliseconds.
    pub duration_ms: i64,
    /// The framerate of the video stream, in frames per second.
    pub framerate: f64,
    /// Configures certain aspects of this [`Player`].
    pub options: PlayerOptions,
    audio_stream_info: StreamInfo,
    subtitle_stream_info: StreamInfo,
    message_sender: PlayerMessageSender,
    message_reciever: PlayerMessageReciever,
    video_timer: Timer,
    audio_timer: Timer,
    subtitle_timer: Timer,
    audio_thread: Option<Guard>,
    video_thread: Option<Guard>,
    subtitle_thread: Option<Guard>,
    ctx_ref: egui::Context,
    last_seek_ms: Option<i64>,
    preseek_player_state: Option<PlayerState>,
    #[cfg(feature = "from_bytes")]
    temp_file: Option<NamedTempFile>,
    video_elapsed_ms: Shared<i64>,
    audio_elapsed_ms: Shared<i64>,
    subtitle_elapsed_ms: Shared<i64>,
    video_elapsed_ms_override: Option<i64>,
    // subtitles_queue: SubtitleQueue,
    // current_subtitles: Vec<Subtitle>,
    input_path: String,
}

use chrono::{DateTime, Duration, Utc};
use std::time::UNIX_EPOCH;
fn format_duration(dur: Duration) -> String {
    let dt = DateTime::<Utc>::from(UNIX_EPOCH) + dur;
    if dt.format("%H").to_string().parse::<i64>().unwrap() > 0 {
        dt.format("%H:%M:%S").to_string()
    } else {
        dt.format("%M:%S").to_string()
    }
}

impl FFMpegPlayer {
    /// The elapsed duration of the stream, in milliseconds. This value will won't be truly accurate to the decoders
    /// while seeking, and will instead be overridden with the target seek location (for visual representation purposes).
    pub fn elapsed_ms(&self) -> i64 {
        self.video_elapsed_ms_override
            .as_ref()
            .map(|i| *i)
            .unwrap_or(self.video_elapsed_ms.get())
    }


    pub fn duration_text(&mut self) -> String {
        format!(
            "{} / {}",
            format_duration(Duration::milliseconds(self.elapsed_ms())),
            format_duration(Duration::milliseconds(self.duration_ms))
        )
    }


    

}



/// The possible states of a [`Player`].
#[derive(PartialEq, Clone, Copy, Debug, NoUninit)]
#[repr(u8)]
pub enum PlayerState {
    /// No playback.
    Stopped,
    /// Streams have reached the end of the file.
    EndOfFile,
    /// Stream is seeking.
    SeekingInProgress,
    /// Stream has finished seeking.
    SeekingFinished,
    /// Playback is paused.
    Paused,
    /// Playback is ongoing.
    Playing,
    /// Playback is scheduled to restart.
    Restarting,
}

#[derive(PartialEq, Clone, Copy)]
/// The index of the stream.
pub struct StreamIndex(usize);


type ApplyVideoFrameFn = Box<dyn FnMut(ColorImage) + Send>;
pub struct VideoStreamer {
    video_decoder: ffmpeg::decoder::Video,
    video_stream_index: StreamIndex,
    player_state: Shared<PlayerState>,
    duration_ms: i64,
    input_context: Input,
    video_elapsed_ms: Shared<i64>,
    _audio_elapsed_ms: Shared<i64>,
    apply_video_frame_fn: Option<ApplyVideoFrameFn>,
}

 
 

 

impl FFMpegPlayer {
   
     
}


use ffmpeg_next::frame::Video;

fn video_frame_to_image(frame: Video) -> ColorImage {
    let size = [frame.width() as usize, frame.height() as usize];
    let data = frame.data(0);
    let stride = frame.stride(0);
    let pixel_size_bytes = 3;
    let byte_width: usize = pixel_size_bytes * frame.width() as usize;
    let height: usize = frame.height() as usize;
    let mut pixels = vec![];
    for line in 0..height {
        let begin = line * stride;
        let end = begin + byte_width;
        let data_line = &data[begin..end];
        pixels.extend(
            data_line
                .chunks_exact(pixel_size_bytes)
                .map(|p| Color32::from_rgb(p[0], p[1], p[2])),
        )
    }
    ColorImage { size:size,source_size: Vec2::new(size[0] as f32, size[1] as f32), pixels:pixels }
}