use atomic::Atomic;
use ffmpeg_next as ffmpeg;
use ffmpeg::format::context::input::Input;
use ffmpeg::format::input;
use ffmpeg::Rational;
use egui::{ColorImage,Color32,Ui,Response,Rect,vec2,Shadow,CornerRadius,Spinner,FontId,Align2};
use egui::{TextureHandle,Vec2,TextureOptions};
use timer::{Guard, Timer};
use bytemuck::NoUninit;
use std::sync::{Arc,Mutex,Weak};
use anyhow::Result;


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

#[inline]
fn millisec_approx_eq(a: i64, b: i64) -> bool {
    a.abs_diff(b) < 50
}

use ffmpeg_next::ffi::AV_TIME_BASE;
const AV_TIME_BASE_RATIONAL: Rational = Rational(1, AV_TIME_BASE);
const MILLISEC_TIME_BASE: Rational = Rational(1, 1000);

use ffmpeg_next::Rescale;
fn millisec_to_timestamp(millisec: i64, time_base: Rational) -> i64 {
    millisec.rescale(MILLISEC_TIME_BASE, time_base)
}
fn timestamp_to_millisec(timestamp: i64, time_base: Rational) -> i64 {
    timestamp.rescale(time_base, MILLISEC_TIME_BASE)
}

fn is_ffmpeg_eof_error(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<ffmpeg::Error>(),
        Some(ffmpeg::Error::Eof)
    )
}

fn is_ffmpeg_incomplete_error(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<ffmpeg::Error>(),
        Some(ffmpeg::Error::Other { errno } ) if *errno == ffmpeg_next::error::EAGAIN
    )
}
/// Streams data.
pub trait Streamer: Send {
    /// The associated type of frame used for the stream.
    type Frame;
    /// The associated type after the frame is processed.
    type ProcessedFrame;
    /// Seek to a location within the stream.
    fn seek(&mut self, seek_frac: f32) {
        let target_ms = (seek_frac as f64 * self.duration_ms() as f64) as i64;
        let seek_completed = millisec_approx_eq(target_ms, self.elapsed_ms().get());
        // stop seeking near target so we dont waste cpu cycles
        if !seek_completed {
            let elapsed_ms = self.elapsed_ms().clone();
            let currently_behind_target = || elapsed_ms.get() < target_ms;

            let seeking_backwards = target_ms < self.elapsed_ms().get();
            let target_ts = millisec_to_timestamp(target_ms, ffmpeg_next::rescale::TIME_BASE);

            // TODO: propogate error
            if self.input_context().seek(target_ts, ..target_ts).is_ok() {
                self.decoder().flush();
                let mut previous_elapsed_ms = self.elapsed_ms().get();

                // this drop frame loop lets us refresh until current_ts is accurate
                if seeking_backwards {
                    while !currently_behind_target() {
                        let next_elapsed_ms = self.elapsed_ms().get();
                        if next_elapsed_ms > previous_elapsed_ms {
                            break;
                        }
                        previous_elapsed_ms = next_elapsed_ms;
                        if let Err(e) = self.drop_frames() {
                            if is_ffmpeg_eof_error(&e) {
                                break;
                            }
                        }
                    }
                }

                // // this drop frame loop drops frames until we are at desired
                while currently_behind_target() {
                    if let Err(e) = self.drop_frames() {
                        if is_ffmpeg_eof_error(&e) {
                            break;
                        }
                    }
                }

                // frame preview
                if self.is_primary_streamer() {
                    if let Ok(frame) = self.recieve_next_packet_until_frame() {
                        self.apply_frame(frame)
                    }
                }
            }
        }
        if self.is_primary_streamer() {
            self.player_state().set(PlayerState::SeekingFinished);
        }
    }
    /// The type of data this stream corresponds to.
    fn stream_type(&self) -> Type;
    /// The primary streamer will control most of the state/syncing.
    fn is_primary_streamer(&self) -> bool;
    /// The stream index.
    fn stream_index(&self) -> StreamIndex;
    /// Move to the next stream index, if possible, and return the new_stream_index.
    fn cycle_stream(&mut self) -> StreamIndex;
    /// The elapsed time of this streamer, in milliseconds.
    fn elapsed_ms(&self) -> &Shared<i64>;
    /// The elapsed time of the primary streamer, in milliseconds.
    fn primary_elapsed_ms(&self) -> &Shared<i64>;
    /// The total duration of the stream, in milliseconds.
    fn duration_ms(&self) -> i64;
    /// The streamer's decoder.
    fn decoder(&mut self) -> &mut ffmpeg::decoder::Opened;
    /// The streamer's input context.
    fn input_context(&mut self) -> &mut ffmpeg::format::context::Input;
    /// The streamer's state.
    fn player_state(&self) -> &Shared<PlayerState>;
    /// Output a frame from the decoder.
    fn decode_frame(&mut self) -> Result<Self::Frame>;
    /// Ignore the remainder of this packet.
    fn drop_frames(&mut self) -> Result<()> {
        if self.decode_frame().is_err() {
            self.recieve_next_packet()
        } else {
            self.drop_frames()
        }
    }
    /// Recieve the next packet of the stream.
    fn recieve_next_packet(&mut self) -> Result<()> {
        let StreamIndex(si)  = self.stream_index().clone();
        if let Some(packet) = self.input_context().packets().next() {
            let (stream, packet)= packet;
            let time_base = stream.time_base();
            if stream.index() == si {
                self.decoder().send_packet(&packet)?;
                match packet.dts() {
                    // Don't try to set elasped time off of undefined timestamp values
                    Some(ffmpeg::ffi::AV_NOPTS_VALUE) => (),
                    Some(dts) => {
                        self.elapsed_ms().set(timestamp_to_millisec(dts, time_base));
                    }
                    _ => (),
                }
            }
        } else {
            self.decoder().send_eof()?;
        }
        Ok(())
    }
    /// Reset the stream to its initial state.
    fn reset(&mut self) {
        let beginning: i64 = 0;
        let beginning_seek = beginning.rescale((1, 1), ffmpeg_next::rescale::TIME_BASE);
        let _ = self.input_context().seek(beginning_seek, ..beginning_seek);
        self.decoder().flush();
    }
    /// Keep recieving packets until a frame can be decoded.
    fn recieve_next_packet_until_frame(&mut self) -> Result<Self::ProcessedFrame> {
        match self.recieve_next_frame() {
            Ok(frame_result) => Ok(frame_result),
            Err(e) => {
                // dbg!(&e, is_ffmpeg_incomplete_error(&e));
                if is_ffmpeg_incomplete_error(&e) {
                    self.recieve_next_packet()?;
                    self.recieve_next_packet_until_frame()
                } else {
                    Err(e)
                }
            }
        }
    }
    /// Process a decoded frame.
    fn process_frame(&mut self, frame: Self::Frame) -> Result<Self::ProcessedFrame>;
    /// Apply a processed frame
    fn apply_frame(&mut self, _frame: Self::ProcessedFrame) {}
    /// Decode and process a frame.
    fn recieve_next_frame(&mut self) -> Result<Self::ProcessedFrame> {
        match self.decode_frame() {
            Ok(decoded_frame) => self.process_frame(decoded_frame),
            Err(e) => Err(e),
        }
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

use egui::{Image,Sense};
use egui::load::SizedTexture;


#[derive(PartialEq, Clone, Copy)]
/// The index of the stream.
pub struct StreamIndex(usize);

use std::sync::mpsc::{channel,Receiver,Sender};



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
use ffmpeg_next::software::scaling::{context::Context as ScaleContext, flag::Flags};

impl Streamer for VideoStreamer {
    type Frame = Video;
    type ProcessedFrame = ColorImage;
    fn stream_type(&self) -> Type {
        Type::Video
    }
    fn is_primary_streamer(&self) -> bool {
        true
    }
    fn stream_index(&self) -> StreamIndex {
        self.video_stream_index
    }
    fn cycle_stream(&mut self) -> StreamIndex {
        StreamIndex(0)
    }
    fn decoder(&mut self) -> &mut ffmpeg::decoder::Opened {
        &mut self.video_decoder.0
    }
    fn input_context(&mut self) -> &mut ffmpeg::format::context::Input {
        &mut self.input_context
    }
    fn elapsed_ms(&self) -> &Shared<i64> {
        &self.video_elapsed_ms
    }
    fn primary_elapsed_ms(&self) -> &Shared<i64> {
        &self.video_elapsed_ms
    }
    fn duration_ms(&self) -> i64 {
        self.duration_ms
    }
    fn player_state(&self) -> &Shared<PlayerState> {
        &self.player_state
    }
    fn decode_frame(&mut self) -> Result<Self::Frame> {
        let mut decoded_frame = Video::empty();
        self.video_decoder.receive_frame(&mut decoded_frame)?;
        Ok(decoded_frame)
    }
    fn apply_frame(&mut self, frame: Self::ProcessedFrame) {
        if let Some(apply_video_frame_fn) = self.apply_video_frame_fn.as_mut() {
            apply_video_frame_fn(frame)
        }
    }
    fn process_frame(&mut self, frame: Self::Frame) -> Result<Self::ProcessedFrame> {
        let mut rgb_frame = Video::empty();
        let mut scaler = ScaleContext::get(
            frame.format(),
            frame.width(),
            frame.height(),
            ffmpeg_next::format::Pixel::RGB24,
            frame.width(),
            frame.height(),
            Flags::BILINEAR,
        )?;
        scaler.run(&frame, &mut rgb_frame)?;

        let image = video_frame_to_image(rgb_frame);
        Ok(image)
    }
}


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
    // audio_stream_info: StreamInfo,
    // subtitle_stream_info: StreamInfo,
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
use std::time::{SystemTime, UNIX_EPOCH};
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

    fn reset(&mut self) {
        self.last_seek_ms = None;
        self.video_elapsed_ms_override = None;
        self.video_elapsed_ms.set(0);
        self.audio_elapsed_ms.set(0);
        self.video_streamer.lock().unwrap().reset();
        // if let Some(audio_decoder) = self.audio_streamer.as_mut() {
        //     audio_decoder.lock().reset();
        // }
    }
    
    fn set_state(&mut self, new_state: PlayerState) {
        self.player_state.set(new_state)
    }
    /// Pause the stream.
    pub fn pause(&mut self) {
        self.set_state(PlayerState::Paused)
    }
    /// Resume the stream from a paused state.
    pub fn resume(&mut self) {
        self.set_state(PlayerState::Playing)
    }
    /// Stop the stream.
    pub fn stop(&mut self) {
        self.set_state(PlayerState::Stopped);
        self.video_thread = None;
        self.audio_thread = None;
        self.reset()
    }
    fn duration_frac(&mut self) -> f32 {
        self.elapsed_ms() as f32 / self.duration_ms as f32
    }

    /// Seek to a location in the stream.
    pub fn seek(&mut self, seek_frac: f32) {
        let current_state = self.player_state.get();
        if !matches!(current_state, PlayerState::SeekingInProgress) {
            match current_state {
                PlayerState::Stopped | PlayerState::EndOfFile => {
                    self.preseek_player_state = Some(PlayerState::Paused);
                    self.start();
                }
                PlayerState::Paused | PlayerState::Playing => {
                    self.preseek_player_state = Some(current_state);
                }
                _ => (),
            }

            let video_streamer = self.video_streamer.clone();
            // let mut audio_streamer = self.audio_streamer.clone();
            // let mut subtitle_streamer = self.subtitle_streamer.clone();
            // let subtitle_queue = self.subtitles_queue.clone();

            self.last_seek_ms = Some((seek_frac as f64 * self.duration_ms as f64) as i64);
            self.set_state(PlayerState::SeekingInProgress);

            // if let Some(audio_streamer) = audio_streamer.take() {
            //     std::thread::spawn(move || {
            //         audio_streamer.lock().seek(seek_frac);
            //     });
            // };
            // if let Some(subtitle_streamer) = subtitle_streamer.take() {
            //     self.current_subtitles.clear();
            //     std::thread::spawn(move || {
            //         subtitle_queue.lock().clear();
            //         subtitle_streamer.lock().seek(seek_frac);
            //     });
            // };
            std::thread::spawn(move || {
                video_streamer.lock().unwrap().seek(seek_frac);
            });
        }
    }




    

    fn spawn_timers(&mut self) {
        let mut texture_handle = self.texture_handle.clone();
        let texture_options = self.options.texture_options;
        let ctx = self.ctx_ref.clone();
        let wait_duration = Duration::milliseconds((1000. / 60.0) as i64);
        println!("wait_duration:{:?}",wait_duration);
        // let wait_duration = Duration::milliseconds((1000. /60.0) as i64);
        fn play<T: Streamer>(streamer: &Weak<Mutex<T>>) {
            println!("当前时间:{:?}",SystemTime::now());
            if let Some(streamer) = streamer.upgrade() {
                if let Ok(mut streamer) = streamer.try_lock() {
                    if (streamer.player_state().get() == PlayerState::Playing)
                        && streamer.primary_elapsed_ms().get() >= streamer.elapsed_ms().get()
                    {
                        match streamer.recieve_next_packet_until_frame() {
                            Ok(frame) => streamer.apply_frame(frame),
                            Err(e) => {
                                if is_ffmpeg_eof_error(&e) && streamer.is_primary_streamer() {
                                    streamer.player_state().set(PlayerState::EndOfFile)
                                }
                            }
                        }
                    }
                }
            }
        }
        let mut vs = self.video_streamer.lock().unwrap();
        vs.apply_video_frame_fn = Some(Box::new(move |frame| {
            texture_handle.set(frame, texture_options)
        }));

        let video_streamer_ref = Arc::downgrade(&self.video_streamer);

        let video_timer_guard = self.video_timer.schedule_repeating(wait_duration, move || {
            play(&video_streamer_ref);
            ctx.request_repaint();
        });

        self.video_thread = Some(video_timer_guard);

        // if let Some(audio_decoder) = self.audio_streamer.as_ref() {
        //     let audio_decoder_ref = Arc::downgrade(audio_decoder);
        //     let audio_timer_guard = self
        //         .audio_timer
        //         .schedule_repeating(Duration::zero(), move || play(&audio_decoder_ref));
        //     self.audio_thread = Some(audio_timer_guard);
        // }

        // if let Some(subtitle_decoder) = self.subtitle_streamer.as_ref() {
        //     let subtitle_decoder_ref = Arc::downgrade(subtitle_decoder);
        //     let subtitle_timer_guard = self
        //         .subtitle_timer
        //         .schedule_repeating(wait_duration, move || play(&subtitle_decoder_ref));
        //     self.subtitle_thread = Some(subtitle_timer_guard);
        // }
    }
    /// Start the stream.
    pub fn start(&mut self) {
        self.stop();
        self.spawn_timers();
        self.resume();
    }


     /// Process player state updates. This function must be called for proper function
    /// of the player. This function is already included in  [`Player::ui`] or
    /// [`Player::ui_at`].
    pub fn process_state(&mut self) {
        let mut reset_stream = false;

        match self.player_state.get() {
            PlayerState::EndOfFile => {
                if self.options.looping {
                    reset_stream = true;
                } else {
                    self.player_state.set(PlayerState::Stopped);
                }
            }
            PlayerState::Playing => {
                // for subtitle in self.current_subtitles.iter_mut() {
                //     subtitle.remaining_duration_ms -=
                //         self.ctx_ref.input(|i| (i.stable_dt * 1000.) as i64);
                // }
                // self.current_subtitles
                //     .retain(|s| s.remaining_duration_ms > 0);
                // if let Some(mut queue) = self.subtitles_queue.try_lock() {
                //     if queue.len() > 1 {
                //         self.current_subtitles.push(queue.pop_front().unwrap());
                //     }
                // }
            }
            state @ (PlayerState::SeekingInProgress | PlayerState::SeekingFinished) => {
                if self.last_seek_ms.is_some() {
                    let last_seek_ms = *self.last_seek_ms.as_ref().unwrap();
                    if matches!(state, PlayerState::SeekingFinished) {
                        if let Some(previeous_player_state) = self.preseek_player_state {
                            self.set_state(previeous_player_state)
                        }
                        self.video_elapsed_ms_override = None;
                        self.last_seek_ms = None;
                    } else {
                        self.video_elapsed_ms_override = Some(last_seek_ms);
                    }
                } else {
                    self.video_elapsed_ms_override = None;
                }
            }
            PlayerState::Restarting => reset_stream = true,
            _ => (),
        }
        if let Ok(message) = self.message_reciever.try_recv() {
            match message {
                PlayerMessage::StreamCycled(stream_type) => match stream_type {
                    Type::Audio => {
                        // self.audio_stream_info.cycle()
                    },
                    Type::Subtitle => {
                        // self.current_subtitles.clear();
                        // self.subtitle_stream_info.cycle();
                    }
                    _ => unreachable!(),
                },
            }
        }
        if reset_stream {
            self.reset();
            self.resume();
        }
    }

    /// Create the [`egui::Image`] for the video frame.
    pub fn generate_frame_image(&self, size: Vec2) -> Image<'_> {
        Image::new(SizedTexture::new(self.texture_handle.id(), size)).sense(Sense::click())
    }

    /// Draw the video frame with a specific rect (without controls). Make sure to call [`Player::process_state`].
    pub fn render_frame(&self, ui: &mut Ui, size: Vec2) -> Response {
        ui.add(self.generate_frame_image(size))
    }

    /// Draw the video frame (without controls). Make sure to call [`Player::process_state`].
    pub fn render_frame_at(&self, ui: &mut Ui, rect: Rect) -> Response {
        ui.put(rect, self.generate_frame_image(rect.size()))
    }

    /// Draw the video frame and player controls and process state changes.
    pub fn ui(&mut self, ui: &mut Ui, size: Vec2) -> egui::Response {
        let frame_response = self.render_frame(ui, size);
        self.render_controls(ui, &frame_response);
        // self.render_subtitles(ui, &frame_response);
        self.process_state();
        frame_response
    }

    /// Draw the video frame and player controls with a specific rect, and process state changes.
    pub fn ui_at(&mut self, ui: &mut Ui, rect: Rect) -> egui::Response {
        let frame_response = self.render_frame_at(ui, rect);
        self.render_controls(ui, &frame_response);
        // self.render_subtitles(ui, &frame_response);
        self.process_state();
        frame_response
    }

    /// Draw the player controls. Make sure to call [`Player::process_state()`]. Unless you are explicitly
    /// drawing something in between the video frames and controls, it is probably better to use
    /// [`Player::ui`] or [`Player::ui_at`].
    pub fn render_controls(&mut self, ui: &mut Ui, frame_response: &Response) {
        let hovered = ui.rect_contains_pointer(frame_response.rect);
        let player_state = self.player_state.get();
        let currently_seeking = matches!(
            player_state,
            PlayerState::SeekingInProgress | PlayerState::SeekingFinished
        );
        let is_stopped = matches!(player_state, PlayerState::Stopped);
        let is_paused = matches!(player_state, PlayerState::Paused);
        let animation_time = 0.2;
        let seekbar_anim_frac = ui.ctx().animate_bool_with_time(
            frame_response.id.with("seekbar_anim"),
            hovered || currently_seeking || is_paused || is_stopped,
            animation_time,
        );

        if seekbar_anim_frac <= 0. {
            return;
        }

        let seekbar_width_offset = 20.;
        let fullseekbar_width = frame_response.rect.width() - seekbar_width_offset;

        let seekbar_width = fullseekbar_width * self.duration_frac();

        let seekbar_offset = 20.;
        let seekbar_pos =
            frame_response.rect.left_bottom() + vec2(seekbar_width_offset / 2., -seekbar_offset);
        let seekbar_height = 3.;
        let mut fullseekbar_rect =
            Rect::from_min_size(seekbar_pos, vec2(fullseekbar_width, seekbar_height));

        let mut seekbar_rect =
            Rect::from_min_size(seekbar_pos, vec2(seekbar_width, seekbar_height));
        let seekbar_interact_rect = fullseekbar_rect.expand(10.);

        let seekbar_response = ui.interact(
            seekbar_interact_rect,
            frame_response.id.with("seekbar"),
            Sense::click_and_drag(),
        );

        let seekbar_hovered = seekbar_response.hovered();
        let seekbar_hover_anim_frac = ui.ctx().animate_bool_with_time(
            frame_response.id.with("seekbar_hover_anim"),
            seekbar_hovered || currently_seeking,
            animation_time,
        );

        if seekbar_hover_anim_frac > 0. {
            let new_top = fullseekbar_rect.top() - (3. * seekbar_hover_anim_frac);
            fullseekbar_rect.set_top(new_top);
            seekbar_rect.set_top(new_top);
        }

        let seek_indicator_anim = ui.ctx().animate_bool_with_time(
            frame_response.id.with("seek_indicator_anim"),
            currently_seeking,
            animation_time,
        );

        if currently_seeking {
            let seek_indicator_shadow = Shadow {
                offset: [10, 20],
                blur: 15,
                spread: 0,
                color: Color32::from_black_alpha(96).linear_multiply(seek_indicator_anim),
            };
            let spinner_size = 20. * seek_indicator_anim;
            ui.painter()
                .add(seek_indicator_shadow.as_shape(frame_response.rect, CornerRadius::ZERO));
            ui.put(
                Rect::from_center_size(frame_response.rect.center(), Vec2::splat(spinner_size)),
                Spinner::new().size(spinner_size),
            );
        }

        if seekbar_hovered || currently_seeking {
            if let Some(hover_pos) = seekbar_response.hover_pos() {
                if seekbar_response.clicked() || seekbar_response.dragged() {
                    let seek_frac = ((hover_pos - frame_response.rect.left_top()).x
                        - seekbar_width_offset / 2.)
                        .max(0.)
                        .min(fullseekbar_width)
                        / fullseekbar_width;
                    seekbar_rect.set_right(
                        hover_pos
                            .x
                            .min(fullseekbar_rect.right())
                            .max(fullseekbar_rect.left()),
                    );
                    if is_stopped {
                        self.start()
                    }
                    self.seek(seek_frac);
                }
            }
        }
        let text_color = Color32::WHITE.linear_multiply(seekbar_anim_frac);

        let pause_icon = if is_paused {
            "▶"
        } else if is_stopped {
            "◼"
        } else if currently_seeking {
            "↔"
        } else {
            "⏸"
        };
        let audio_volume_frac = self.options.audio_volume.get() / self.options.max_audio_volume;
        let sound_icon = if audio_volume_frac > 0.7 {
            "🔊"
        } else if audio_volume_frac > 0.4 {
            "🔉"
        } else if audio_volume_frac > 0. {
            "🔈"
        } else {
            "🔇"
        };

        let icon_font_id = FontId {
            size: 16.,
            ..Default::default()
        };

        let subtitle_icon = "💬";
        let stream_icon = "🔁";
        let icon_margin = 5.;
        let text_y_offset = -7.;
        let sound_icon_offset = vec2(-5., text_y_offset);
        let sound_icon_pos = fullseekbar_rect.right_top() + sound_icon_offset;

        let stream_index_icon_offset = vec2(-30., text_y_offset + 1.);
        let stream_icon_pos = fullseekbar_rect.right_top() + stream_index_icon_offset;

        let contraster_alpha: u8 = 100;
        let pause_icon_offset = vec2(3., text_y_offset);
        let pause_icon_pos = fullseekbar_rect.left_top() + pause_icon_offset;

        let duration_text_offset = vec2(25., text_y_offset);
        let duration_text_pos = fullseekbar_rect.left_top() + duration_text_offset;
        let duration_text_font_id = FontId {
            size: 14.,
            ..Default::default()
        };

        let shadow = Shadow {
            offset: [10, 20],
            blur: 15,
            spread: 0,
            color: Color32::from_black_alpha(25).linear_multiply(seekbar_anim_frac),
        };

        let mut shadow_rect = frame_response.rect;
        shadow_rect.set_top(shadow_rect.bottom() - seekbar_offset - 10.);

        let fullseekbar_color = Color32::GRAY.linear_multiply(seekbar_anim_frac);
        let seekbar_color = Color32::WHITE.linear_multiply(seekbar_anim_frac);

        ui.painter()
            .add(shadow.as_shape(shadow_rect, CornerRadius::ZERO));

        ui.painter().rect_filled(
            fullseekbar_rect,
            CornerRadius::ZERO,
            fullseekbar_color.linear_multiply(0.5),
        );
        ui.painter()
            .rect_filled(seekbar_rect, CornerRadius::ZERO, seekbar_color);
        ui.painter().text(
            pause_icon_pos,
            Align2::LEFT_BOTTOM,
            pause_icon,
            icon_font_id.clone(),
            text_color,
        );

        ui.painter().text(
            duration_text_pos,
            Align2::LEFT_BOTTOM,
            self.duration_text(),
            duration_text_font_id,
            text_color,
        );

        if seekbar_hover_anim_frac > 0. {
            ui.painter().circle_filled(
                seekbar_rect.right_center(),
                7. * seekbar_hover_anim_frac,
                seekbar_color,
            );
        }

        if frame_response.clicked() {
            let mut reset_stream = false;
            let mut start_stream = false;

            match self.player_state.get() {
                PlayerState::Stopped => start_stream = true,
                PlayerState::EndOfFile => reset_stream = true,
                PlayerState::Paused => self.player_state.set(PlayerState::Playing),
                PlayerState::Playing => self.player_state.set(PlayerState::Paused),
                _ => (),
            }

            if reset_stream {
                self.reset();
                self.resume();
            } else if start_stream {
                self.start();
            }
        }

        // let is_audio_cyclable = self.audio_stream_info.is_cyclable();
        // let is_subtitle_cyclable = self.audio_stream_info.is_cyclable();

        // if is_audio_cyclable || is_subtitle_cyclable {
        //     let stream_icon_rect = ui.painter().text(
        //         stream_icon_pos,
        //         Align2::RIGHT_BOTTOM,
        //         stream_icon,
        //         icon_font_id.clone(),
        //         text_color,
        //     );
        //     let stream_icon_hovered = ui.rect_contains_pointer(stream_icon_rect);
        //     let mut stream_info_hovered = false;
        //     let mut cursor = stream_icon_rect.right_top() + vec2(0., 5.);
        //     let cursor_offset = vec2(3., 15.);
        //     let stream_anim_id = frame_response.id.with("stream_anim");
        //     let mut stream_anim_frac: f32 = ui
        //         .ctx()
        //         .memory_mut(|m| *m.data.get_temp_mut_or_default(stream_anim_id));

        //     let mut draw_row = |stream_type: Type| {
        //         let text = match stream_type {
        //             Type::Audio => format!("{} {}", sound_icon, self.audio_stream_info),
        //             Type::Subtitle => format!("{} {}", subtitle_icon, self.subtitle_stream_info),
        //             _ => unreachable!(),
        //         };

        //         let text_position = cursor - cursor_offset;
        //         let text_galley =
        //             ui.painter()
        //                 .layout_no_wrap(text.clone(), icon_font_id.clone(), text_color);

        //         let background_rect =
        //             Rect::from_min_max(text_position - text_galley.size(), text_position)
        //                 .expand(5.);

        //         let background_color =
        //             Color32::from_black_alpha(contraster_alpha).linear_multiply(stream_anim_frac);

        //         ui.painter()
        //             .rect_filled(background_rect, Rounding::same(5.), background_color);

        //         if ui.rect_contains_pointer(background_rect.expand(5.)) {
        //             stream_info_hovered = true;
        //         }

        //         if ui
        //             .interact(
        //                 background_rect,
        //                 frame_response.id.with(&text),
        //                 Sense::click(),
        //             )
        //             .clicked()
        //         {
        //             match stream_type {
        //                 Type::Audio => self.cycle_audio_stream(),
        //                 Type::Subtitle => self.cycle_subtitle_stream(),
        //                 _ => unreachable!(),
        //             };
        //         };

        //         let text_rect = ui.painter().text(
        //             text_position,
        //             Align2::RIGHT_BOTTOM,
        //             text,
        //             icon_font_id.clone(),
        //             text_color.linear_multiply(stream_anim_frac),
        //         );

        //         cursor.y = text_rect.top();
        //     };

        //     if stream_anim_frac > 0. {
        //         if is_audio_cyclable {
        //             draw_row(Type::Audio);
        //         }
        //         if is_subtitle_cyclable {
        //             draw_row(Type::Subtitle);
        //         }
        //     }

        //     stream_anim_frac = ui.ctx().animate_bool_with_time(
        //         stream_anim_id,
        //         stream_icon_hovered || (stream_info_hovered && stream_anim_frac > 0.),
        //         animation_time,
        //     );

        //     ui.ctx()
        //         .memory_mut(|m| m.data.insert_temp(stream_anim_id, stream_anim_frac));
        // }

        // if self.audio_streamer.is_some() {
            // let sound_icon_rect = ui.painter().text(
            //     sound_icon_pos,
            //     Align2::RIGHT_BOTTOM,
            //     sound_icon,
            //     icon_font_id.clone(),
            //     text_color,
            // );
            // if ui
            //     .interact(
            //         sound_icon_rect,
            //         frame_response.id.with("sound_icon_sense"),
            //         Sense::click(),
            //     )
            //     .clicked()
            // {
            //     if self.options.audio_volume.get() != 0. {
            //         self.options.audio_volume.set(0.)
            //     } else {
            //         self.options
            //             .audio_volume
            //             .set(self.options.max_audio_volume / 2.)
            //     }
            // }

            // let sound_slider_outer_height = 75.;

            // let mut sound_slider_rect = sound_icon_rect;
            // sound_slider_rect.set_bottom(sound_icon_rect.top() - icon_margin);
            // sound_slider_rect.set_top(sound_slider_rect.top() - sound_slider_outer_height);

            // let sound_slider_interact_rect = sound_slider_rect.expand(icon_margin);
            // let sound_hovered = ui.rect_contains_pointer(sound_icon_rect);
            // let sound_slider_hovered = ui.rect_contains_pointer(sound_slider_interact_rect);
            // let sound_anim_id = frame_response.id.with("sound_anim");
            // let mut sound_anim_frac: f32 = ui
            //     .ctx()
            //     .memory_mut(|m| *m.data.get_temp_mut_or_default(sound_anim_id));
            // sound_anim_frac = ui.ctx().animate_bool_with_time(
            //     sound_anim_id,
            //     sound_hovered || (sound_slider_hovered && sound_anim_frac > 0.),
            //     0.2,
            // );
            // ui.ctx()
            //     .memory_mut(|m| m.data.insert_temp(sound_anim_id, sound_anim_frac));
            // let sound_slider_bg_color =
            //     Color32::from_black_alpha(contraster_alpha).linear_multiply(sound_anim_frac);
            // let sound_bar_color =
            //     Color32::from_white_alpha(contraster_alpha).linear_multiply(sound_anim_frac);
            // let mut sound_bar_rect = sound_slider_rect;
            // sound_bar_rect
            //     .set_top(sound_bar_rect.bottom() - audio_volume_frac * sound_bar_rect.height());

            // ui.painter()
            //     .rect_filled(sound_slider_rect, Rounding::same(5.), sound_slider_bg_color);

            // ui.painter()
            //     .rect_filled(sound_bar_rect, Rounding::same(5.), sound_bar_color);
            // let sound_slider_resp = ui.interact(
            //     sound_slider_rect,
            //     frame_response.id.with("sound_slider_sense"),
            //     Sense::click_and_drag(),
            // );
            // if sound_anim_frac > 0. && sound_slider_resp.clicked() || sound_slider_resp.dragged() {
            //     if let Some(hover_pos) = ui.ctx().input(|i| i.pointer.hover_pos()) {
            //         let sound_frac = 1.
            //             - ((hover_pos - sound_slider_rect.left_top()).y
            //                 / sound_slider_rect.height())
            //             .clamp(0., 1.);
            //         self.options
            //             .audio_volume
            //             .set(sound_frac * self.options.max_audio_volume);
            //     }
            // }
        // }
    }


    #[cfg(feature = "from_bytes")]
    /// Create a new [`Player`] from input bytes.
    pub fn from_bytes(ctx: &egui::Context, input_bytes: &[u8]) -> Result<Self> {
        let mut file = tempfile::Builder::new().tempfile()?;
        file.write_all(input_bytes)?;
        let path = file.path().to_string_lossy().to_string();
        let mut slf = Self::new(ctx, &path)?;
        slf.temp_file = Some(file);
        Ok(slf)
    }


    fn cycle_stream<T: Streamer + 'static>(&self, mut streamer: Option<&Arc<Mutex<T>>>) {
        if let Some(streamer) = streamer.take() {
            let message_sender = self.message_sender.clone();
            let streamer = streamer.clone();
            std::thread::spawn(move || {
                let mut streamer = streamer.lock().unwrap();
                streamer.cycle_stream();
                message_sender.send(PlayerMessage::StreamCycled(streamer.stream_type()))
            });
        };
    }

    fn try_set_texture_handle(&mut self) -> Result<TextureHandle> {
        match self.video_streamer.lock().unwrap().recieve_next_packet_until_frame() {
            Ok(first_frame) => {
                let texture_handle = self.ctx_ref.load_texture(
                    "vidstream",
                    first_frame,
                    self.options.texture_options,
                );
                let texture_handle_clone = texture_handle.clone();
                self.texture_handle = texture_handle;
                Ok(texture_handle_clone)
            }
            Err(e) => Err(e),
        }
    }

    /// Create a new [`Player`].
    pub fn new(ctx: &egui::Context, input_path: &String) -> Result<Self> {
        let input_context = input(&input_path)?;
        let video_stream = input_context
            .streams()
            .best(Type::Video)
            .ok_or(ffmpeg::Error::StreamNotFound)?;
        let video_stream_index = StreamIndex(video_stream.index());

        let video_elapsed_ms = Shared::new(0);
        let audio_elapsed_ms = Shared::new(0);
        let player_state = Shared::new(PlayerState::Stopped);

        let video_context =
            ffmpeg::codec::context::Context::from_parameters(video_stream.parameters())?;
        let video_decoder = video_context.decoder().video()?;
        let framerate = (video_stream.avg_frame_rate().numerator() as f64)
            / video_stream.avg_frame_rate().denominator() as f64;

        let (width, height) = (video_decoder.width(), video_decoder.height());
        let size = Vec2::new(width as f32, height as f32);
        let duration_ms = timestamp_to_millisec(input_context.duration(), AV_TIME_BASE_RATIONAL); // in sec
        // let duration_ms = 16;
        let stream_decoder = VideoStreamer {
            apply_video_frame_fn: None,
            duration_ms,
            video_decoder,
            video_stream_index,
            _audio_elapsed_ms: audio_elapsed_ms.clone(),
            video_elapsed_ms: video_elapsed_ms.clone(),
            input_context,
            player_state: player_state.clone(),
        };
        let options = PlayerOptions::default();
        let texture_handle =
            ctx.load_texture("vidstream", ColorImage::example(), options.texture_options);
        let (message_sender, message_reciever) = std::sync::mpsc::channel();
        let mut streamer = Self {
            input_path: input_path.clone(),
            // audio_streamer: None,
            // subtitle_streamer: None,
            video_streamer: Arc::new(Mutex::new(stream_decoder)),
            // subtitle_stream_info: StreamInfo::new(),
            // audio_stream_info: StreamInfo::new(),
            framerate,
            video_timer: Timer::new(),
            audio_timer: Timer::new(),
            subtitle_timer: Timer::new(),
            subtitle_elapsed_ms: Shared::new(0),
            preseek_player_state: None,
            video_thread: None,
            subtitle_thread: None,
            audio_thread: None,
            texture_handle,
            player_state,
            message_sender,
            message_reciever,
            video_elapsed_ms,
            audio_elapsed_ms,
            size,
            last_seek_ms: None,
            duration_ms,
            options,
            video_elapsed_ms_override: None,
            ctx_ref: ctx.clone(),
            // subtitles_queue: Arc::new(Mutex::new(VecDeque::new())),
            // current_subtitles: Vec::new(),
            #[cfg(feature = "from_bytes")]
            temp_file: None,
        };
        
         
        loop {
            if let Ok(_texture_handle) = streamer.try_set_texture_handle() {
                break;
            }
        }

        Ok(streamer)
    }


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