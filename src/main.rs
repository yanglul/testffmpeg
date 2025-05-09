
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
use play::audio;

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
use std::time::Duration;
use ffmpeg_next::util::format::sample;
use cpal::SizedSample;
use bytemuck::Pod;

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
    // 启动解码线程
    
    let (device, stream_config,sample) = init_cpal();
    println!("audio_decoder.format(){:?}",audio_decoder.format());
    println!("stream_config.sample_format(){:?}",stream_config.sample_format());

    // A buffer to hold audio samples
    let buf = HeapRb::<f32>::new(88200);
    let (mut producer, mut consumer) = buf.split();
    

    // Set up a resampler for the audio
    let mut resampler = ResamplingContext::get(
        audio_decoder.format(),
        audio_decoder.channel_layout(),
        audio_decoder.rate(),
        
        sample,
        audio_decoder.channel_layout(),
        stream_config.sample_rate().0
    ).unwrap();

    audio::audio_play(stream_config, &device, consumer);



    tokio::spawn(async move {
        player::Player::start(ictx,audio_packet_sender,video_packet_sender,video_stream_index,audio_stream_index,video_decoder,audio_decoder);
    });
    loop{

        match audio_packet_receiver.try_recv(){
            Ok((apacket,ist_index))=>{
                let expected_bytes =
                apacket.samples() * apacket.channels() as usize * core::mem::size_of::<T>();
                let cpal_sample_data: &[T] =
                    bytemuck::cast_slice(&audio_frame.data(0)[..expected_bytes]);
                
                },
            _=>{}
        }


        match video_packet_receiver.try_recv(){
            Ok((vpacket,ist_index))=>{
                // 对视频帧进行缩放
                let mut rgb_frame = Video::empty();
                scaler.run(&vpacket, &mut rgb_frame).unwrap();

                // 渲染到 SDL2 画布
                canvas.clear();
                let texture_creator = canvas.texture_creator();
                let mut texture = texture_creator.create_texture_target(
                    sdl2::pixels::PixelFormatEnum::RGB24,
                    rgb_frame.width() as u32,
                    rgb_frame.height() as u32,
                ).unwrap();
                texture.update(None, &rgb_frame.data(ist_index), rgb_frame.stride(ist_index) as usize).unwrap();
                canvas.copy(&texture, None, None).unwrap();
                canvas.present();
                thread::sleep(Duration::from_millis(16));
            },
            _=>{}
        }

    }




 

    
}

fn init_cpal() -> (cpal::Device, cpal::SupportedStreamConfig,ffmpeg_next::util::format::sample::Sample) {
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
    let config  = supported_config_range.with_max_sample_rate();
    let output_channel_layout = match config.channels() {
            1 => ffmpeg_next::util::channel_layout::ChannelLayout::MONO,
            2 => ffmpeg_next::util::channel_layout::ChannelLayout::STEREO,
            _ => todo!(),
        };

    let sample = match config.sample_format() {
            cpal::SampleFormat::U8 => ffmpeg_next::util::format::sample::Sample::U8(
                ffmpeg_next::util::format::sample::Type::Packed,
            ),
            cpal::SampleFormat::F32 => ffmpeg_next::util::format::sample::Sample::F32(
                ffmpeg_next::util::format::sample::Type::Packed,
            ),
            format @ _ => todo!("unsupported cpal output format {:#?}", format),
        };

    // Pick the best (highest) sample rate
    (device,config ,sample)
}