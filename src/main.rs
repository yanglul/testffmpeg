
extern crate ffmpeg_next as ffmpeg;
extern crate sdl2;


use std::collections::HashMap;
use std::env;
 

use ffmpeg::{
    codec::{self, traits::Decoder}, decoder::{self, new}, encoder, format, frame::{self, Audio, Video}, log, media, picture, Dictionary, Packet, Rational,
};

use ffmpeg_next::software::scaling::{context::Context as ScaleContext, flag::Flags};

use ffmpeg_next:: codec::  Context as CodecContext;
use ffmpeg_next::util::format::pixel::Pixel;

use sdl2::video::{Window, WindowContext};
use sdl2::render::Canvas;
use sdl2::pixels::Color;
use sdl2::audio::{AudioCallback, AudioDevice, AudioSpec, AudioSpecDesired, AudioQueue};

use std::time::{Duration, Instant};

extern crate byteorder;
use byteorder::{LittleEndian, ReadBytesExt};
use std::io::Cursor;
fn convert_u8_to_i16(data: &[u8]) -> Vec<i16> {
    let mut cursor = Cursor::new(data);
    let mut result = Vec::new();
    
    // 每个音频样本为 2 字节 (16 位)
    while let Ok(sample) = cursor.read_i16::<LittleEndian>() {
        result.push(sample);
    }
    
    result
}
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
    let audio_subsystem = sdl_context.audio().unwrap();


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


    // 设置音频播放
    let desired_spec = AudioSpecDesired {
        freq: Some(44_100),
        channels: Some(2),
        samples: Some(4096),
    };

     

    // Open an audio device
    let device  = audio_subsystem
        .open_queue::<i16, _>(None, &desired_spec)
        .unwrap();


    // let device = audio_subsystem.open_playback(None, &desired_spec, get_callback).unwrap();



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
     
     
    let mut flag = true;
    device.resume();
    let mut i_iter = ictx.packets();
    'main_loop: loop{
        let (stream,mut packet) = i_iter.next().expect("遍历文件失败"); 
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
                last_video_pts = video_decoded_frame.timestamp().unwrap() as f64 / video_decoded_frame.aspect_ratio().denominator() as f64;
            }
        } else if packet.stream() == audio_stream_index {
            packet.rescale_ts(stream.time_base(), audio_decoder.time_base());
            audio_decoder.send_packet(&packet).expect("解码音频失败");
            let mut audio_frame = Audio::empty();
            if audio_decoder.receive_frame(&mut audio_frame).is_ok() {
                let timestamp = audio_frame.timestamp();
                // 播放音频
                audio_frame.set_pts(timestamp);
                device.queue_audio(convert_u8_to_i16(&audio_frame.data(0)).as_slice() ).expect("加载音频失败");
                last_audio_pts = audio_frame.timestamp().unwrap() as f64 / audio_frame.rate()  as f64;
            }
         }  
        // 控制播放同步
        let now = Instant::now();
        // if last_video_pts > last_audio_pts {
        //     // 等待视频同步
        //     while now.elapsed() < Duration::from_secs_f64(last_video_pts - last_audio_pts) {
        //         std::thread::sleep(Duration::from_millis(10));
        //     }
        // } else if last_audio_pts > last_video_pts {
        //     // 等待音频同步
        //     while now.elapsed() < Duration::from_secs_f64(last_audio_pts - last_video_pts) {
        //         std::thread::sleep(Duration::from_millis(10));
        //     }
        // }

         
    
    }


     
}
