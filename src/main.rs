
extern crate sdl2;
use std::env;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ffmpeg_next::{decoder::{new}, format, frame::{self, Audio, Video}, log, media
};

use ffmpeg_next::software::scaling::{context::Context as ScaleContext, flag::Flags};
use ffmpeg_next::format::Sample as FFmpegSample;
use ffmpeg_next::format::sample::Type as SampleType;
use ffmpeg_next:: codec::  Context as CodecContext;
use ffmpeg_next::util::format::pixel::Pixel;

mod play;
mod player;
use std::thread;
use sdl2::video::{Window};
use sdl2::pixels::Color;
use std::time::{ Instant};
extern crate byteorder;
extern crate cpal;
use ffmpeg_next::software::resampling::{context::Context as ResamplingContext};
use cpal::{ SampleFormat};
use ringbuf::{traits::*,HeapRb};

fn main() {
    let input_file = env::args().nth(1).expect("missing input file");
   
    ffmpeg_next::init().unwrap();
    log::set_level(log::Level::Info);

    let mut ictx = format::input(&input_file).unwrap();
    // let mut octx = format::output(&output_file).unwrap();

    format::context::input::dump(&ictx, 0, Some(&input_file));
    // 查找视频流
    let video_stream_index = ictx
        .streams()
        .best(media::Type::Video)
        .map(|stream| stream.index()).expect("视频索引获取失败");
    

    let video_stream = ictx.stream(video_stream_index).unwrap();
    let codec_ctx = CodecContext::from_parameters(video_stream.parameters()).unwrap();
 

    // 打开解码器
    let mut video_decoder =  codec_ctx.decoder().video().unwrap();

    // 查找音频流
    let audio_stream_index = ictx
        .streams()
        .best(media::Type::Audio)
        .map(|stream| stream.index()).expect("音频索引获取失败");


    let audio_stream = ictx.stream(audio_stream_index).unwrap();
    let audio_codec_ctx = CodecContext::from_parameters(audio_stream.parameters()).unwrap();
    let mut audio_decoder = audio_codec_ctx.decoder().audio().unwrap();

    // 初始化 SDL2
    let sdl_context = sdl2::init().unwrap();
    let video_subsystem = sdl_context.video().unwrap();
    // let audio_subsystem = sdl_context.audio().unwrap();


    // 创建 SDL2 窗口和画布
    let window: Window = video_subsystem
        .window("FFmpeg + SDL2 Video Player", 1920, 1080)
        .position_centered()
        .build()
        .unwrap();


    let mut canvas  = window.into_canvas().build().unwrap();
    canvas.set_draw_color(Color::BLACK);
    canvas.clear();
    canvas.present();
    let mut event_pump = sdl_context.event_pump().unwrap();
    let mut i = 0;
    let (video_packet_sender, video_packet_receiver) = smol::channel::unbounded();
    let (audio_packet_sender, audio_packet_receiver) = smol::channel::unbounded();

    // 获取视频帧的缩放上下文
    let mut scaler: ScaleContext = ScaleContext::get(
        Pixel::YUV420P,
        video_decoder.width(),
        video_decoder.height(),
        Pixel::RGB24,
        video_decoder.width(),
        video_decoder.height(),
        Flags::BILINEAR,
    ).unwrap(); 

 

    
}

fn init_cpal() -> (cpal::Device, cpal::SupportedStreamConfig) {
    let device = cpal::default_host()
        .default_output_device()
        .expect("no output device available");

    // Create an output stream for the audio so we can play it
    // NOTE: If system doesn't support the file's sample rate, the program will panic when we try to play,
    //       so we'll need to resample the audio to a supported config
    let supported_config_range = device.supported_output_configs()
        .expect("error querying audio output configs")
        .next()
        .expect("no supported audio config found");

    // Pick the best (highest) sample rate
    (device, supported_config_range.with_max_sample_rate())
}