
extern crate ffmpeg_next as ffmpeg;
extern crate sdl2;

use std::collections::HashMap;
use std::env;
 

use ffmpeg::{
    codec::{self, traits::Decoder}, decoder, encoder, format, frame::{self, Video,Audio}, log, media, picture, Dictionary, Packet, Rational,
};

use ffmpeg_next::software::scaling::{context::Context as ScaleContext, flag::Flags};

use ffmpeg_next:: codec::  Context as CodecContext;
use ffmpeg_next::util::format::pixel::Pixel;

use sdl2::video::{Window, WindowContext};
use sdl2::render::Canvas;
use sdl2::pixels::Color;
use sdl2::audio::{AudioSpecDesired, AudioQueue};

use std::time::{Duration, Instant};


const DEFAULT_X264_OPTS: &str = "preset=medium";

 

fn main() {
    let input_file = env::args().nth(1).expect("missing input file");
    // let output_file = env::args().nth(2).expect("missing output file");
   
    ffmpeg::init().unwrap();
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
    let mut video_decoder:decoder::Video =  codec_ctx.decoder().video().unwrap();

    // 查找音频流
    let audio_stream_index = ictx
        .streams()
        .into_iter()
        .position(|stream| stream.parameters().medium() == media::Type::Audio)
        .unwrap();


    let audio_stream = ictx.stream(audio_stream_index).unwrap();
    let audio_codec_ctx = CodecContext::from_parameters(audio_stream.parameters()).unwrap();
    let mut audio_decoder = audio_codec_ctx.decoder().audio().unwrap();

    // 初始化 SDL2
    let sdl_context = sdl2::init().unwrap();
    let video_subsystem = sdl_context.video().unwrap();
    let audio_subsystem = sdl_context.audio().unwrap();


    // 创建 SDL2 窗口和画布
    let window: Window = video_subsystem
        .window("FFmpeg + SDL2 Video Player", 800, 600)
        .position_centered()
        .build()
        .unwrap();


    let mut canvas  = window.into_canvas().build().unwrap();
    canvas.set_draw_color(Color::BLACK);
    canvas.clear();
    canvas.present();
    let mut event_pump = sdl_context.event_pump().unwrap();
    let mut i = 0;


    // 设置音频播放
    let desired_spec = AudioSpecDesired {
        freq: Some(44_100),
        channels: Some(2),
        samples: Some(1024),
    };

    let audio_queue = audio_subsystem
        .open_queue(None, &desired_spec)
        .unwrap();


    // 获取视频帧的缩放上下文
    let mut scaler = ScaleContext::get(
        Pixel::YUV420P,
        video_decoder.width(),
        video_decoder.height(),
        Pixel::RGB24,
        video_decoder.width(),
        video_decoder.height(),
        Flags::BILINEAR,
    ).unwrap();
    

    //加载视频信息
 




    // 播放音视频
    let mut packet = Packet::empty();
    let mut last_video_pts = 0.0;
    let mut last_audio_pts = 0.0;
 

    for(stream,mut packet) in ictx.packets(){
        // 读取视频帧
        let ist_index = stream.index();
        if packet.stream() == video_stream_index {
            video_decoder.send_packet(&packet).expect("解码视频失败");
            let mut video_decoded_frame = Video::empty();
            if video_decoder.receive_frame(&mut video_decoded_frame).is_ok() {
                // 对视频帧进行缩放
                let mut rgb_frame = Video::empty();
                scaler.run(&video_decoded_frame, &mut rgb_frame).unwrap();

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
            }
        } else if packet.stream() == audio_stream_index {
            audio_decoder.send_packet(&packet).expect("解码音频失败");
            let mut audio_frame = Audio::empty();
            if audio_decoder.receive_frame(&mut audio_frame).is_ok() {
                // 播放音频
                audio_queue.queue_audio(audio_frame.data(0)).unwrap(); 
            }
        }
    }


     
}
